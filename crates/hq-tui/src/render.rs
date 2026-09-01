//! Borrowed responsive Ratatui renderer.

use std::{fmt::Write as _, sync::Arc};

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    UiActivityStatus, UiAgent, UiAgentAssignmentPhase, UiAgentAttentionReason, UiAgentModal,
    UiAgentProjectAssignment, UiAgentStatus, UiCompletedItemPresentation, UiConnectionState,
    UiConversationActivityKind, UiConversationAuthor, UiConversationEntry,
    UiConversationEntryGeometry, UiConversationEntryPresentation,
    UiConversationViewportObservation, UiFocus, UiHelpPage, UiHumanIssue,
    UiHumanMembershipEvidence, UiHumanMembershipStatus, UiHumanState, UiInteractionKind,
    UiInteractionModal, UiMailboxAction, UiMailboxDraftPane, UiMailboxDraftTarget, UiMailboxModal,
    UiManagedSessionAction, UiManagedSessionOutcome, UiMessageDelivery, UiMessageState, UiModel,
    UiNewChoice, UiNewModal, UiProjectAction, UiProjectAssignedAgentStatus,
    UiProjectCreationChoice, UiProjectFolderAction, UiProjectFolderOwnership, UiProjectFormField,
    UiProjectInteraction, UiProjectLifecycle, UiProjectManagementAction, UiProjectOutcome,
    UiProjectRecoverySummary, UiProjectSummaryFocus, UiProjectThread, UiProjectWorkspaceLevel,
    UiProvider, UiRow, UiRowKind, UiRowState, UiSection, UiTechnicalSection, UiTheme, UiThemeRole,
    message_markdown::{MessageRenderCache, RenderedMessage},
    model::WIDE_WIDTH,
};

const MINIMUM_WIDTH: u16 = 40;
const MINIMUM_HEIGHT: u16 = 10;
const NAVIGATION_SELECTED_MARKER: &str = " › ";
const NAVIGATION_UNSELECTED_MARKER: &str = "   ";
const NAVIGATION_TRAILING_PADDING: usize = 1;
const NAVIGATION_BORDER_WIDTH: usize = 1;
const INBOX_LIST_MIN_WIDTH: u16 = 24;
const INBOX_LIST_PREFERRED_WIDTH: u16 = 32;
const INBOX_LIST_MAX_WIDTH: u16 = 36;
const CONVERSATION_PREFERRED_MIN_WIDTH: u16 = 48;

fn navigation_width() -> u16 {
    let marker_width = NAVIGATION_SELECTED_MARKER
        .width()
        .max(NAVIGATION_UNSELECTED_MARKER.width());
    let label_width = UiSection::ALL
        .into_iter()
        .map(|section| section_label(section).width())
        .max()
        .unwrap_or_default();
    let width = marker_width
        .saturating_add(label_width)
        .saturating_add(NAVIGATION_TRAILING_PADDING)
        .saturating_add(NAVIGATION_BORDER_WIDTH);
    u16::try_from(width).unwrap_or(u16::MAX)
}

fn inbox_list_width(available: u16) -> u16 {
    let width_preserving_conversation =
        available.saturating_sub(CONVERSATION_PREFERRED_MIN_WIDTH + 1);
    width_preserving_conversation
        .clamp(INBOX_LIST_MIN_WIDTH, INBOX_LIST_PREFERRED_WIDTH)
        .min(INBOX_LIST_MAX_WIDTH)
}

/// Reusable bounded presentation state owned by a terminal renderer.
#[derive(Default)]
pub struct UiRenderCache {
    messages: MessageRenderCache,
    conversation_viewport: Option<UiConversationViewportObservation>,
}

impl UiRenderCache {
    /// Creates an empty bounded render cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Renders the complete model without mutation or I/O.
pub fn render(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme) {
    let _ = render_with_cache(frame, model, theme, &mut UiRenderCache::new());
}

/// Renders the complete model while reusing terminal-owned presentation artifacts.
pub fn render_with_cache(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    cache: &mut UiRenderCache,
) -> Option<UiConversationViewportObservation> {
    cache.conversation_viewport = None;
    let area = frame.area();
    frame.render_widget(Block::new().style(theme.style(UiThemeRole::Screen)), area);
    if area.width < MINIMUM_WIDTH || area.height < MINIMUM_HEIGHT {
        render_too_small(frame, model, theme, area);
        return None;
    }

    let [header, content, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(area);
    render_header(frame, model, theme, header);
    if area.width >= WIDE_WIDTH {
        render_wide_content(frame, model, theme, content, cache);
    } else {
        render_compact_content(frame, model, theme, content, cache);
    }
    render_footer(frame, model, theme, footer);
    render_new_modal(frame, model, theme, content);
    render_mailbox_modal(frame, model, theme, content);
    render_agent_modal(frame, model, theme, content);
    render_project_interaction(frame, model, theme, content, true);
    render_interaction_modal(frame, model, theme, content);
    render_help(frame, model, theme, content);
    cache.conversation_viewport.take()
}

fn render_interaction_modal(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    available: Rect,
) {
    let Some(dialog) = model.interaction_modal() else {
        return;
    };
    let (interaction, submitting, selected, text) = match dialog {
        UiInteractionModal::Prompt {
            interaction,
            selected,
            text,
        } => (interaction, false, *selected, text.as_str()),
        UiInteractionModal::Submitting { interaction } => (interaction, true, 0, ""),
    };
    let width = available.width.saturating_sub(4).clamp(1, 82);
    let height = available.height.clamp(1, 22);
    let area = Rect {
        x: available.x + available.width.saturating_sub(width) / 2,
        y: available.y + available.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    let title = match interaction.kind {
        UiInteractionKind::Question => format!(" {} needs an answer ", interaction.agent_name),
        UiInteractionKind::CommandApproval => " Command approval needed ".to_owned(),
        UiInteractionKind::FileApproval => " File approval needed ".to_owned(),
        UiInteractionKind::Permission => " Permission needed ".to_owned(),
        UiInteractionKind::McpUrl => " Link approval needed ".to_owned(),
        UiInteractionKind::McpForm => format!(" {} needs information ", interaction.agent_name),
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .title(Span::styled(title, theme.style(UiThemeRole::ModalTitle)))
        .border_style(theme.style(UiThemeRole::ModalBorder))
        .style(theme.style(UiThemeRole::ModalSurface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![
        Line::styled(
            interaction.project_name.as_ref().map_or_else(
                || format!("{} is waiting for you", interaction.agent_name),
                |project| {
                    format!(
                        "{} is waiting for you while working on {project}",
                        interaction.agent_name
                    )
                },
            ),
            theme.style(UiThemeRole::Accent),
        ),
        Line::default(),
        Line::from(interaction.prompt.clone()),
        Line::default(),
    ];
    for (index, choice) in interaction.choices.iter().enumerate() {
        lines.push(Line::styled(
            format!(
                "{} {}",
                if index == selected { "›" } else { " " },
                choice.label
            ),
            if index == selected {
                theme.style(UiThemeRole::SelectionFocused)
            } else {
                theme.style(UiThemeRole::Text)
            },
        ));
    }
    if interaction.allow_text {
        lines.push(Line::default());
        lines.push(Line::from(format!("Your answer: {text}")));
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            "Technical: provider={} · session={} · operation={}{}",
            interaction.provider,
            interaction.session,
            short_identity(interaction.operation_id),
            interaction
                .project_id
                .map_or_else(String::new, |project_id| {
                    format!(" · project={}", short_identity(project_id))
                })
        ),
        theme.style(UiThemeRole::TextMuted),
    ));
    lines.push(Line::from(if submitting {
        "Sending your response…"
    } else if interaction.allow_text {
        "Type an answer or choose with ↑/↓ · Enter send · Esc cancel"
    } else {
        "↑/↓ choose · Enter confirm · Esc cancel"
    }));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[allow(clippy::too_many_lines)]
fn render_new_modal(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, available: Rect) {
    let Some(interaction) = model.new_modal() else {
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
    let (title, mut lines) = match interaction {
        UiNewModal::Launcher { selected } => {
            let choices = [
                (
                    UiNewChoice::ProjectWork,
                    "Work with an agent on a project",
                    "choose the work and the agent who should handle it",
                ),
                (
                    UiNewChoice::DirectMessage,
                    "Send a direct message",
                    "contact a reachable agent or, later, another person",
                ),
                (
                    UiNewChoice::PersonalNote,
                    "Write a personal note",
                    "save something only for yourself",
                ),
            ];
            let mut lines = vec![
                Line::from("What would you like to do?"),
                Line::from("HQ will guide you through only the choices that intent needs."),
                Line::default(),
            ];
            for (choice, label, detail) in choices {
                lines.push(project_choice_line(
                    theme,
                    label,
                    detail,
                    *selected == choice,
                ));
            }
            (" New… ", lines)
        }
        UiNewModal::ChooseProject {
            projects,
            selected,
            create_new,
        } => {
            let mut lines = vec![
                Line::from("Which project is this work for?"),
                Line::from("A project records the work and the folders or resources it owns."),
                Line::default(),
            ];
            for project in projects {
                lines.push(project_choice_line(
                    theme,
                    &project.name,
                    project
                        .resources
                        .iter()
                        .find(|resource| resource.primary)
                        .or_else(|| project.resources.first())
                        .map_or("no folder recorded", |resource| {
                            resource.display_path.as_str()
                        }),
                    *selected == Some(project.project_id) && !*create_new,
                ));
            }
            lines.push(project_choice_line(
                theme,
                "Create a project",
                "record a folder and its ownership first",
                *create_new,
            ));
            (" Choose project ", lines)
        }
        UiNewModal::ChooseAgent {
            project,
            agents,
            selected,
            create_new,
        } => {
            let mut lines = vec![
                Line::from(format!("Who should work on {}?", project.name)),
                Line::from(
                    "Unassigned agents are listed first. HQ will not silently move an agent.",
                ),
                Line::default(),
            ];
            for agent in agents {
                lines.push(project_choice_line(
                    theme,
                    agent.names.first().map_or("Unnamed agent", String::as_str),
                    &agent_status_label(&agent.status),
                    *selected == Some(agent.agent_id) && !*create_new,
                ));
            }
            lines.push(project_choice_line(
                theme,
                "Create an agent",
                "give a new worker a durable name",
                *create_new,
            ));
            (" Choose agent ", lines)
        }
        UiNewModal::ChooseProvider {
            project,
            agent,
            providers,
            provider,
        } => {
            let available = providers.iter().filter(|choice| choice.available).count();
            let mut lines = vec![
                Line::from(format!(
                    "Project: {} · Agent: {}",
                    project.name,
                    agent.names.first().map_or("Unnamed agent", String::as_str)
                )),
                Line::from(
                    if project
                        .threads
                        .iter()
                        .any(|thread| thread.agent_id == agent.agent_id)
                        || project
                            .assignment
                            .as_ref()
                            .is_some_and(|assignment| assignment.runnable)
                    {
                        "HQ will continue the compatible saved project conversation."
                    } else {
                        "HQ will prepare a project conversation, then open a new message."
                    },
                ),
                Line::default(),
            ];
            if available > 1 {
                lines.push(Line::styled(
                    format!(
                        "Agent service: {}",
                        provider_choice_label(providers, provider)
                    ),
                    selected_style(theme, true),
                ));
                lines.push(Line::from(
                    "Use ↑/↓ or j/k to choose; HQ uses the default when possible.",
                ));
                lines.push(Line::default());
            } else if available == 1 {
                lines.push(Line::from(format!(
                    "Agent service: {} (selected automatically)",
                    provider_choice_label(providers, provider)
                )));
                lines.push(Line::default());
            } else if !project
                .threads
                .iter()
                .any(|thread| thread.agent_id == agent.agent_id)
            {
                lines.push(Line::styled(
                    "No agent service is available. Configure one before continuing.",
                    theme.style(UiThemeRole::Warning),
                ));
                lines.push(Line::default());
            }
            (" Start project work ", lines)
        }
        UiNewModal::ReviewProject {
            project,
            agent,
            provider,
            resumes_existing,
            moves_project,
            submitting,
        } => (
            " Review project work ",
            vec![
                Line::from("HQ is ready to do the following:"),
                Line::default(),
                Line::from(format!("Project: {}", project.name)),
                Line::from(format!(
                    "Agent: {}",
                    agent.names.first().map_or("Unnamed agent", String::as_str)
                )),
                Line::from(format!("Agent service: {provider}")),
                Line::from(format!(
                    "Conversation: {}",
                    if *resumes_existing {
                        "continue saved"
                    } else {
                        "create for this project"
                    }
                )),
                Line::from(if *moves_project {
                    "Assignment: move this project's work to the selected agent"
                } else {
                    "Assignment: keep or create this project's assignment"
                }),
                Line::default(),
                Line::from(if *submitting {
                    "Preparing project work safely…"
                } else {
                    "Enter confirm · Esc edit"
                }),
            ],
        ),
        UiNewModal::AgentUnavailable {
            project,
            agent,
            competing_project,
            ..
        } => (
            " Agent already assigned ",
            vec![
                Line::styled(
                    format!(
                        "{} is assigned to {}.",
                        agent.names.first().map_or("This agent", String::as_str),
                        competing_project
                    ),
                    theme.style(UiThemeRole::Attention),
                ),
                Line::from(format!(
                    "HQ will not silently take the agent for {}.",
                    project.name
                )),
                Line::default(),
                Line::from(
                    "Enter inspect the competing project and choose its explicit handoff path.",
                ),
                Line::from("Esc choose another agent."),
            ],
        ),
        UiNewModal::ProjectUnavailable {
            project,
            competing_project,
            reason,
        } => (
            " Project needs attention ",
            vec![
                Line::styled(
                    format!("{} cannot start work yet: {reason}.", project.name),
                    theme.style(UiThemeRole::Attention),
                ),
                Line::from(competing_project.as_ref().map_or_else(
                    || "Inspect the project to see the conflicting folder evidence.".to_owned(),
                    |name| format!("The competing project is {name}."),
                )),
                Line::default(),
                Line::from("Enter inspect this project · Esc choose another project"),
            ],
        ),
        UiNewModal::Working {
            project,
            agent,
            stage,
        } => (
            " Starting project work ",
            vec![
                Line::from(format!("Project: {project} · Agent: {agent}")),
                Line::default(),
                Line::from(stage.as_str()),
                Line::from("Your project and agent choices are retained if setup needs attention."),
            ],
        ),
    };
    if !matches!(
        interaction,
        UiNewModal::ReviewProject { .. }
            | UiNewModal::AgentUnavailable { .. }
            | UiNewModal::ProjectUnavailable { .. }
            | UiNewModal::Working { .. }
    ) {
        lines.push(Line::default());
        lines.push(Line::from("↑/↓ or j/k choose · Enter continue · Esc back"));
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new().style(theme.style(UiThemeRole::ModalSurface)),
        area,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.style(UiThemeRole::Text))
            .block(
                Block::bordered()
                    .title(Span::styled(title, theme.style(UiThemeRole::ModalTitle)))
                    .border_style(theme.style(UiThemeRole::ModalBorder)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_help(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, available: Rect) {
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
    frame.render_widget(
        Block::new().style(theme.style(UiThemeRole::ModalSurface)),
        area,
    );
    let (title, lines, text_role) = match page {
        UiHelpPage::Context => (
            " Help ",
            contextual_help_lines(model, theme),
            UiThemeRole::Text,
        ),
        UiHelpPage::Technical => (
            " Technical details ",
            technical_help_lines(model, theme),
            UiThemeRole::TextTechnical,
        ),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.style(text_role))
            .block(
                Block::bordered()
                    .title(Span::styled(title, theme.style(UiThemeRole::ModalTitle)))
                    .border_style(theme.style(UiThemeRole::ModalBorder)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn contextual_help_lines(model: &UiModel, theme: &UiTheme) -> Vec<Line<'static>> {
    if let Some(lines) = dialog_help_lines(model, theme) {
        return lines;
    }
    let mut lines = vec![
        Line::styled("What this is", theme.style(UiThemeRole::Heading)),
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
    if matches!(
        model.human_state(),
        Some(UiHumanState::NeedsAttention(
            UiHumanIssue::NoAccountSelected
        ))
    ) {
        lines.push(Line::styled(
            "Already have an HQ account?",
            theme.style(UiThemeRole::Heading),
        ));
        lines.push(Line::from(
            "Join it with: hq human join ABSOLUTE_INVITATION_PATH",
        ));
    }
    lines.push(Line::styled(
        "Available actions",
        theme.style(UiThemeRole::Heading),
    ));
    lines.extend(section_help_actions(model));
    lines.push(Line::from(
        "q quit · t technical details · F1/?/Esc close help",
    ));
    lines
}

fn dialog_help_lines(model: &UiModel, theme: &UiTheme) -> Option<Vec<Line<'static>>> {
    let (title, purpose) = if let Some(dialog) = model.new_modal() {
        match dialog {
            UiNewModal::Launcher { .. } => (
                "Help for New…",
                "Choose the kind of thing you want to do. HQ will then ask only for information that choice needs.",
            ),
            UiNewModal::ChooseProject { .. } => (
                "Help for choosing a project",
                "A project describes a piece of work and records which folders or other resources it owns.",
            ),
            UiNewModal::ChooseAgent { .. } => (
                "Help for choosing an agent",
                "An agent is a named worker. Unassigned agents can begin this project without moving other work.",
            ),
            UiNewModal::ChooseProvider { .. } => (
                "Help for choosing an agent service",
                "HQ chooses an available service automatically when it can. Choose from the available services when there is more than one.",
            ),
            UiNewModal::ReviewProject { .. } => (
                "Help for reviewing project work",
                "Check the project, agent, service, and instruction before HQ changes an assignment or sends anything.",
            ),
            UiNewModal::AgentUnavailable { .. } => (
                "Help for an assigned agent",
                "HQ will not move an agent away from other work without an explicit handoff decision.",
            ),
            UiNewModal::ProjectUnavailable { .. } => (
                "Help for a project conflict",
                "A folder or assignment is already claimed. Inspect the named project before deciding what should own it.",
            ),
            UiNewModal::Working { .. } => (
                "Help while HQ prepares work",
                "HQ is waiting for authoritative confirmation before it sends your retained instruction.",
            ),
        }
    } else if model.project_interaction().is_some() {
        (
            "Help for this project dialog",
            "The dialog explains the pending project or resource change. Nothing material changes until its confirmation step.",
        )
    } else if model.agent_modal().is_some() {
        (
            "Help for this agent dialog",
            "The dialog changes a named worker or one of its saved conversations. Technical identities stay in details.",
        )
    } else if model.mailbox_modal().is_some() {
        (
            "Help for this message dialog",
            "Choose a recipient or write the message. Draft text is retained if delivery needs attention.",
        )
    } else if model.mailbox_draft().is_some() {
        (
            "Help for writing a message",
            "This draft belongs to the Inbox workspace. HQ saves it as you type and keeps it here if sending needs attention.",
        )
    } else {
        return None;
    };
    Some(vec![
        Line::styled(title, theme.style(UiThemeRole::Heading)),
        Line::from(purpose),
        Line::default(),
        Line::from("Follow the labels and the action guide at the bottom of the dialog."),
        Line::from("t — technical details · F1 / Esc — close help"),
    ])
}

fn technical_help_lines(model: &UiModel, theme: &UiTheme) -> Vec<Line<'static>> {
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
        lines.extend(technical_human_lines(model, issue, theme));
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
            theme.style(UiThemeRole::Warning),
        ));
        lines.push(Line::from(format!("Recovery action: {}", failure.action)));
    } else {
        lines.push(Line::from("Recovery evidence: none"));
    }
    lines.push(Line::from("t — contextual help · ? / Esc — close help"));
    lines
}

fn technical_human_lines(
    model: &UiModel,
    issue: &UiHumanIssue,
    theme: &UiTheme,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        format!("Human recovery code: {}", human_issue_code(issue)),
        theme.style(UiThemeRole::Warning),
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
                    "↑/↓ or j/k — select message · Enter — details · Esc — close conversation",
                ));
                actions.push(Line::from(
                    "r — reply · a — archive · u — restore · PgDn — load more",
                ));
            } else if model.selected_row_data().is_some() {
                actions.push(Line::from("Enter — open selected conversation"));
            }
            actions.push(Line::from(
                "n — New… · d — direct message · N — personal note",
            ));
        }
        UiSection::Agents => {
            actions.push(Line::from(if model.selected_row_data().is_some() {
                "n — New… · Enter — inspect · c — create · / — search"
            } else {
                "n — New… · c — create agent · / — search"
            }));
        }
        UiSection::Projects => {
            actions.push(Line::from(match model.project_workspace_level() {
                UiProjectWorkspaceLevel::List if model.selected_row_data().is_some() => {
                    "Enter — collaborate · l/→ — summary · c — create · / — search"
                }
                UiProjectWorkspaceLevel::List => "c — create project · / — search",
                UiProjectWorkspaceLevel::Summary => {
                    "↑/↓ or j/k — choose · Enter — open · h/← — Projects"
                }
                UiProjectWorkspaceLevel::Manage => {
                    "↑/↓ or j/k — choose action · h/← — project summary"
                }
                UiProjectWorkspaceLevel::Folders => {
                    "↑/↓ or j/k — choose action · Tab — choose folder · h/← — manage"
                }
            }));
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
fn render_project_interaction(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    available: Rect,
    overlay: bool,
) {
    let Some(interaction) = model.project_interaction() else {
        return;
    };
    let bounded_confirmation = matches!(
        interaction,
        UiProjectInteraction::ConfirmRemoveResource { .. }
            | UiProjectInteraction::ConfirmClose { .. }
            | UiProjectInteraction::ConfirmArchive { .. }
    );
    if overlay != bounded_confirmation {
        return;
    }
    let area = if overlay {
        let width = available.width.saturating_sub(4).clamp(1, 88);
        let height = available.height.clamp(1, 22);
        Rect {
            x: available.x + available.width.saturating_sub(width) / 2,
            y: available.y + available.height.saturating_sub(height) / 2,
            width,
            height,
        }
    } else {
        available
    };
    if overlay {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::new().style(theme.style(UiThemeRole::ModalSurface)),
            area,
        );
    }
    let inner_width = area.width.saturating_sub(2);
    let (title, lines) = match interaction {
        UiProjectInteraction::ChooseCreation { selected } => (
            " Create project ",
            vec![
                Line::from("How should this project's first folder be created?"),
                Line::default(),
                project_choice_line(
                    theme,
                    "Use an existing folder",
                    "recommended · record its ownership in HQ",
                    *selected == UiProjectCreationChoice::ExistingFolder,
                ),
                project_choice_line(
                    theme,
                    "Create an isolated Git worktree",
                    "optional advanced · create a branch and separate folder",
                    *selected == UiProjectCreationChoice::IsolatedWorktree,
                ),
                Line::default(),
                Line::from(
                    "HQ tracks folder ownership; it does not take over Git or filesystem maintenance.",
                ),
                Line::from("↑/↓ or j/k choose · Enter continue · Esc cancel"),
            ],
        ),
        UiProjectInteraction::Search { query } => (
            " Search projects ",
            vec![
                text_field_line(
                    theme,
                    inner_width,
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
        UiProjectInteraction::CreateExisting {
            name,
            brief,
            path,
            field,
            submitting,
        } => {
            let mut lines = Vec::new();
            push_project_text_field(
                theme,
                inner_width,
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
                theme,
                inner_width,
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
                theme,
                inner_width,
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
        UiProjectInteraction::CreateWorktree {
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
                theme,
                inner_width,
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
                theme,
                inner_width,
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
                theme,
                inner_width,
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
                theme,
                inner_width,
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
                theme,
                inner_width,
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
                theme,
                inner_width,
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
        UiProjectInteraction::AddResource {
            project,
            path,
            make_primary,
            submitting,
        } => {
            let mut lines = vec![Line::from(format!("Project: {}", project.name))];
            push_project_text_field(
                theme,
                inner_width,
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
                theme,
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
                "Tab/Shift-Tab field · ↑/↓ or j/k change choice · Enter preview · Esc cancel"
            }));
            (" Add a folder or resource ", lines)
        }
        UiProjectInteraction::ReplaceResource {
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
                theme,
                inner_width,
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
        UiProjectInteraction::ConfirmRemoveResource {
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
        UiProjectInteraction::Activate {
            project,
            agents,
            providers,
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
                theme,
                inner_width,
                project,
                agents,
                providers,
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
        UiProjectInteraction::Handoff {
            project,
            agents,
            providers,
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
                theme,
                inner_width,
                project,
                agents,
                providers,
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
        UiProjectInteraction::ConfirmClose {
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
                Line::styled("Folder release check", theme.style(UiThemeRole::Accent)),
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
                        theme.style(UiThemeRole::Error),
                    ));
                }
            }
            lines.push(Line::default());
            lines.push(project_choice_line(
                theme,
                "I understand",
                yes_no(*confirmed),
                model.project_field_is_focused(UiProjectFormField::Confirmation),
            ));
            if let Some(error) = model.project_field_error(UiProjectFormField::Confirmation) {
                lines.push(Line::styled(
                    format!("  {error}"),
                    theme.style(UiThemeRole::Error),
                ));
            }
            lines.push(project_choice_line(
                theme,
                "Override safety check",
                yes_no(*force),
                model.project_field_is_focused(UiProjectFormField::Force),
            ));
            if let Some(error) = model.project_field_error(UiProjectFormField::Force) {
                lines.push(Line::styled(
                    format!("  {error}"),
                    theme.style(UiThemeRole::Error),
                ));
            }
            lines.push(Line::from(
                "Closing keeps folders, files, worktrees, and branches on disk.",
            ));
            lines.push(Line::from(if *submitting {
                "Closing the project safely…"
            } else {
                "Tab/Shift-Tab field · ↑/↓ or j/k change choice · Enter close · Esc cancel"
            }));
            (" Confirm project close ", lines)
        }
        UiProjectInteraction::ConfirmArchive {
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
        UiProjectInteraction::Outcome { result } => {
            let mut lines = vec![Line::from(project_action_label(&result.action))];
            match &result.outcome {
                UiProjectOutcome::Completed { .. } => lines.push(Line::from("Done")),
                UiProjectOutcome::Running { stage } => {
                    lines.push(Line::from("HQ is still finishing this change."));
                    lines.push(Line::from(format!("Current stage: {stage}")));
                }
                UiProjectOutcome::Rejected { .. } => {
                    lines.push(Line::styled(
                        "HQ could not make this change.",
                        theme.style(UiThemeRole::Error),
                    ));
                    lines.push(Line::from(
                        "Review the technical reason, correct the problem, and try again.",
                    ));
                }
                UiProjectOutcome::Reconcilable { warning, .. } => {
                    lines.push(Line::styled(
                        "HQ could not confirm whether the change finished.",
                        theme.style(UiThemeRole::Warning),
                    ));
                    if let Some(warning) = warning {
                        lines.push(Line::from(format!("Technical kind: {}", warning.kind)));
                        lines.push(Line::from(format!(
                            "Kept worktree {} on branch {}.",
                            warning.destination, warning.branch
                        )));
                    }
                    lines.push(Line::from(
                        "HQ retained this request for recovery; do not repeat it manually.",
                    ));
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
                        lines.push(Line::styled(
                            continuation,
                            theme.style(UiThemeRole::Success),
                        ));
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
                            theme.style(UiThemeRole::Error),
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
                                theme.style(UiThemeRole::Error),
                            ));
                        }
                        if matches!(result.action, UiProjectAction::PreviewCreateExisting { .. }) {
                            lines.push(Line::from("Esc edit the folder path"));
                        }
                    }
                }
            }
            if matches!(
                result.outcome,
                UiProjectOutcome::Rejected { .. } | UiProjectOutcome::Reconcilable { .. }
            ) {
                lines.push(Line::default());
                lines.push(Line::styled(
                    if model.viewport().width >= WIDE_WIDTH {
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
                    },
                    theme.style(UiThemeRole::TextTechnical),
                ));
                if let Some(runtime_state) = &result.runtime_state {
                    lines.push(Line::styled(
                        format!(
                            "Technical runtime: {runtime_state}{}",
                            result
                                .runtime_code
                                .as_ref()
                                .map_or_else(String::new, |code| format!("/{code}"))
                        ),
                        theme.style(UiThemeRole::TextTechnical),
                    ));
                }
                match &result.outcome {
                    UiProjectOutcome::Rejected { category, code } => lines.push(Line::styled(
                        format!("Technical reason: {category}/{code}"),
                        theme.style(UiThemeRole::TextTechnical),
                    )),
                    UiProjectOutcome::Reconcilable {
                        stage,
                        category,
                        code,
                        ..
                    } => lines.push(Line::styled(
                        format!("Technical stage and reason: {stage} · {category}/{code}"),
                        theme.style(UiThemeRole::TextTechnical),
                    )),
                    _ => {}
                }
            }
            lines.push(Line::default());
            lines.push(Line::from("Esc close"));
            (
                if matches!(
                    result.action,
                    UiProjectAction::Activate { .. } | UiProjectAction::Handoff { .. }
                ) {
                    " Project work "
                } else {
                    " Project change "
                },
                lines,
            )
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.style(UiThemeRole::Text))
            .block(
                Block::new()
                    .borders(if overlay { Borders::ALL } else { Borders::NONE })
                    .title(Span::styled(title, theme.style(UiThemeRole::ModalTitle)))
                    .border_style(theme.style(UiThemeRole::ModalBorder)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn text_field_line(
    theme: &UiTheme,
    line_width: u16,
    label: &str,
    value: &str,
    cursor: usize,
    selected: bool,
    requirement: &str,
) -> Line<'static> {
    let cursor = cursor.min(value.len());
    let (left, right) = value.split_at(cursor);
    let field_style = theme.style(if selected {
        UiThemeRole::InputFieldFocused
    } else {
        UiThemeRole::InputField
    });
    let label_style = if selected {
        selected_style(theme, true)
    } else {
        theme.style(UiThemeRole::Input)
    };
    let rendered_label = format!("{} {label}:", if selected { '›' } else { ' ' });
    let label_width = UnicodeWidthStr::width(rendered_label.as_str());
    let field_width = usize::from(line_width)
        .saturating_sub(label_width.saturating_add(1))
        .max(1);
    let mut spans = vec![Span::styled(rendered_label, label_style), Span::raw(" ")];
    if value.is_empty() {
        let requirement_width = UnicodeWidthStr::width(requirement).min(field_width);
        let cursor_width = usize::from(selected);
        if selected {
            spans.push(Span::styled(
                " ",
                field_style.patch(theme.style(UiThemeRole::Cursor)),
            ));
        }
        spans.push(Span::styled(
            " ".repeat(
                field_width
                    .saturating_sub(cursor_width)
                    .saturating_sub(requirement_width),
            ),
            field_style,
        ));
        if requirement_width > 0 {
            spans.push(Span::styled(
                display_prefix(requirement, requirement_width).to_owned(),
                field_style,
            ));
        }
    } else if selected {
        let (under_cursor, remainder) = right.chars().next().map_or_else(
            || (" ".to_owned(), ""),
            |character| {
                let next = character.len_utf8();
                (character.to_string(), &right[next..])
            },
        );
        let under_cursor_width = under_cursor
            .chars()
            .next()
            .and_then(UnicodeWidthChar::width)
            .unwrap_or(1)
            .max(1);
        let (under_cursor, caret_width) = if under_cursor_width > field_width {
            (" ".to_owned(), 1)
        } else {
            (under_cursor, under_cursor_width)
        };
        let left_budget = UnicodeWidthStr::width(left).min(field_width.saturating_sub(caret_width));
        let visible_left = display_suffix(left, left_budget);
        let right_budget = field_width
            .saturating_sub(UnicodeWidthStr::width(visible_left))
            .saturating_sub(caret_width);
        let visible_right = display_prefix(remainder, right_budget);
        let content_width = UnicodeWidthStr::width(visible_left)
            .saturating_add(caret_width)
            .saturating_add(UnicodeWidthStr::width(visible_right));
        spans.push(Span::styled(visible_left.to_owned(), field_style));
        spans.push(Span::styled(
            under_cursor,
            field_style.patch(theme.style(UiThemeRole::Cursor)),
        ));
        spans.push(Span::styled(visible_right.to_owned(), field_style));
        spans.push(Span::styled(
            " ".repeat(field_width.saturating_sub(content_width)),
            field_style,
        ));
    } else {
        let visible = display_prefix(value, field_width);
        let visible_width = UnicodeWidthStr::width(visible);
        spans.push(Span::styled(visible.to_owned(), field_style));
        spans.push(Span::styled(
            " ".repeat(field_width.saturating_sub(visible_width)),
            field_style,
        ));
    }
    Line::from(spans)
}

fn display_prefix(value: &str, maximum_width: usize) -> &str {
    let mut width: usize = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width.saturating_add(character_width) > maximum_width {
            break;
        }
        width = width.saturating_add(character_width);
        end = index + character.len_utf8();
    }
    &value[..end]
}

fn display_suffix(value: &str, maximum_width: usize) -> &str {
    let mut width: usize = 0;
    let mut start = value.len();
    for (index, character) in value.char_indices().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width.saturating_add(character_width) > maximum_width {
            break;
        }
        width = width.saturating_add(character_width);
        start = index;
    }
    &value[start..]
}

#[allow(clippy::too_many_arguments)]
fn project_text_field_line(
    theme: &UiTheme,
    line_width: u16,
    model: &UiModel,
    label: &str,
    value: &str,
    field: UiProjectFormField,
    selected: bool,
    required: bool,
) -> Line<'static> {
    text_field_line(
        theme,
        line_width,
        label,
        value,
        model.project_field_cursor(field, value),
        selected,
        if required { "(required)" } else { "(optional)" },
    )
}

#[allow(clippy::too_many_arguments)]
fn push_project_text_field(
    theme: &UiTheme,
    line_width: u16,
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
        theme, line_width, model, label, value, field, selected, required,
    ));
    if let Some(error) = model.project_field_error(field) {
        lines.push(Line::styled(
            format!("  {error}"),
            theme.style(UiThemeRole::Error),
        ));
    } else if selected {
        lines.push(Line::styled(
            format!("  {guidance}"),
            theme.style(UiThemeRole::TextMuted),
        ));
    }
    if path && selected && !value.is_empty() {
        let preview = match model.normalized_path_preview(value) {
            Ok(path) => format!("  Will use: {path}"),
            Err(error) => format!("  {error}"),
        };
        lines.push(Line::styled(preview, theme.style(UiThemeRole::TextMuted)));
    }
}

fn provider_choice_label(providers: &[UiProvider], selected: &str) -> String {
    providers
        .iter()
        .find(|provider| provider.provider == selected)
        .map_or_else(
            || "none available".to_owned(),
            |provider| {
                if provider.configured_default {
                    format!("{} · default", provider.name)
                } else {
                    provider.name.clone()
                }
            },
        )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn project_activation_lines<'value>(
    theme: &UiTheme,
    line_width: u16,
    project: &'value crate::UiProject,
    agents: &'value [UiAgent],
    providers: &'value [UiProvider],
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
        theme,
        "Agent",
        agent,
        field == UiProjectFormField::Agent,
    ));
    if let Some(error) = model.project_field_error(UiProjectFormField::Agent) {
        lines.push(Line::styled(
            format!("  {error}"),
            theme.style(UiThemeRole::Error),
        ));
    }
    lines.push(project_choice_line(
        theme,
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
            theme,
            if wide { "Saved conversation" } else { "Saved" },
            &thread_label,
            field == UiProjectFormField::Thread,
        ));
        if let Some(error) = model.project_field_error(UiProjectFormField::Thread) {
            lines.push(Line::styled(
                format!("  {error}"),
                theme.style(UiThemeRole::Error),
            ));
        }
    }
    let provider_label = if new_session {
        provider_choice_label(providers, provider)
    } else {
        "from the saved conversation".to_owned()
    };
    lines.push(project_choice_line(
        theme,
        "Agent service",
        &provider_label,
        new_session && field == UiProjectFormField::Provider,
    ));
    if let Some(error) = model.project_field_error(UiProjectFormField::Provider) {
        lines.push(Line::styled(
            format!("  {error}"),
            theme.style(UiThemeRole::Error),
        ));
    }
    if new_session && !providers.iter().any(|provider| provider.available) {
        lines.push(Line::styled(
            "  No agent services are available on this device.",
            theme.style(UiThemeRole::Warning),
        ));
        lines.push(Line::from(
            "  Configure or install a provider, then reload HQ.",
        ));
    }
    lines.push(project_text_field_line(
        theme,
        line_width,
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
            theme.style(UiThemeRole::Error),
        ));
    } else if field == UiProjectFormField::Directory && !directory.is_empty() {
        let preview = match model.normalized_path_preview(directory) {
            Ok(path) => format!("  Will use: {path}"),
            Err(error) => format!("  {error}"),
        };
        lines.push(Line::styled(preview, theme.style(UiThemeRole::TextMuted)));
    }
    if let Some((confirmed, force)) = handoff {
        lines.push(project_choice_line(
            theme,
            "I understand",
            yes_no(confirmed),
            field == UiProjectFormField::Confirmation,
        ));
        if let Some(error) = model.project_field_error(UiProjectFormField::Confirmation) {
            lines.push(Line::styled(
                format!("  {error}"),
                theme.style(UiThemeRole::Error),
            ));
        }
        lines.push(project_choice_line(
            theme,
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
            "Tab field · ↑/↓ or j/k change choice · Enter move work"
        } else {
            "Tab field · ↑/↓ or j/k choose · Enter move"
        }
    } else if wide {
        "Tab field · ↑/↓ or j/k change choice · Enter set up work"
    } else {
        "Tab field · ↑/↓ or j/k choose · Enter set up"
    }));
    lines
}

fn project_choice_line(theme: &UiTheme, label: &str, value: &str, selected: bool) -> Line<'static> {
    Line::styled(
        format!("{} {label}: {value}", if selected { '›' } else { ' ' }),
        if selected {
            selected_style(theme, true)
        } else {
            theme.style(UiThemeRole::Input)
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

fn project_action_label(action: &UiProjectAction) -> String {
    match action {
        UiProjectAction::PreviewCreateExisting { name, .. } => {
            format!("Check folder ownership before creating {name}")
        }
        UiProjectAction::CreateExisting { name, .. } => format!("Create {name} from a folder"),
        UiProjectAction::CreateWorktree { name, .. } => {
            format!("Create an isolated worktree for {name}")
        }
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
fn render_agent_modal(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, available: Rect) {
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
    frame.render_widget(
        Block::new().style(theme.style(UiThemeRole::ModalSurface)),
        area,
    );
    let inner_width = area.width.saturating_sub(2);
    let (title, lines) = match interaction {
        UiAgentModal::Search { query } => (
            " Search agents ",
            vec![
                text_field_line(
                    theme,
                    inner_width,
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
            let mut lines = agent_summary(agent, theme);
            lines.push(Line::default());
            lines.push(Line::styled(
                "Saved conversations",
                theme.style(UiThemeRole::Accent),
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
                        selected_style(theme, true)
                    } else {
                        theme.style(UiThemeRole::Input)
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
                "↑/↓ or j/k conversation · s start new · e continue · t stop"
            } else {
                "↑/↓ or j/k choose · s new · e continue · t stop"
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
                theme,
                inner_width,
                "Name",
                name,
                model.agent_field_cursor(name),
                true,
                "(required)",
            )];
            if let Some(error) = model.agent_field_error() {
                lines.push(Line::styled(
                    format!("  {error}"),
                    theme.style(UiThemeRole::Error),
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
                    theme,
                    inner_width,
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
                        theme.style(UiThemeRole::Warning),
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
        UiAgentModal::ManagedProvider {
            providers,
            selected,
            ..
        } => {
            let mut lines = vec![Line::from(
                "Choose the service that will run this agent's new conversation.",
            )];
            lines.push(Line::default());
            for provider in providers {
                let mut status = String::new();
                if provider.configured_default {
                    status.push_str(" · configured default");
                }
                if !provider.available {
                    status.push_str(" · unavailable");
                }
                let is_selected = selected.as_deref() == Some(provider.provider.as_str());
                lines.push(Line::styled(
                    format!(
                        " {} {}{}",
                        if is_selected { '›' } else { ' ' },
                        provider.name,
                        status
                    ),
                    if is_selected {
                        selected_style(theme, true)
                    } else if provider.available {
                        theme.style(UiThemeRole::Input)
                    } else {
                        theme.style(UiThemeRole::TextMuted)
                    },
                ));
            }
            if !providers.iter().any(|provider| provider.available) {
                lines.push(Line::styled(
                    "No agent services are available on this device.",
                    theme.style(UiThemeRole::Attention),
                ));
                lines.push(Line::from(
                    "Configure or install a provider, then reload HQ.",
                ));
            }
            lines.push(Line::default());
            lines.push(Line::from(if selected.is_some() {
                "↑/↓ or j/k choose · Enter continue · Esc cancel"
            } else {
                "Esc cancel"
            }));
            (" Start an agent conversation ", lines)
        }
        UiAgentModal::ConfirmManagedSession { action, .. } => (
            " Switch the agent's conversation? ",
            vec![
                Line::styled(
                    "This differs from the agent's currently saved conversation.",
                    theme.style(UiThemeRole::Warning),
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
                        theme.style(UiThemeRole::Error),
                    ));
                    lines.push(Line::from(format!("Technical reason: {category}/{code}")));
                    lines.push(Line::from(
                        "Reload the agent, choose a current conversation, and try again.",
                    ));
                }
                UiManagedSessionOutcome::Uncertain { reconciliation_id } => {
                    lines.push(Line::styled(
                        "HQ could not confirm whether the change finished.",
                        theme.style(UiThemeRole::Warning),
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
            .style(theme.style(UiThemeRole::Text))
            .block(
                Block::bordered()
                    .title(Span::styled(title, theme.style(UiThemeRole::ModalTitle)))
                    .border_style(theme.style(UiThemeRole::ModalBorder)),
            )
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

fn agent_summary<'agent>(agent: &'agent UiAgent, theme: &UiTheme) -> Vec<Line<'agent>> {
    let mut lines = vec![
        Line::styled(
            agent.names.first().map_or("Unnamed agent", String::as_str),
            theme.style(UiThemeRole::Heading),
        ),
        Line::from(format!("Status: {}", agent_status_label(&agent.status))),
        Line::styled("Technical details", theme.style(UiThemeRole::TextMuted)),
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
fn render_mailbox_modal(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, available: Rect) {
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
    frame.render_widget(
        Block::new().style(theme.style(UiThemeRole::ModalSurface)),
        area,
    );
    match interaction {
        UiMailboxModal::SelectDirect { targets, selected } => {
            let mut lines = vec![Line::styled(
                "Choose who to message",
                theme.style(UiThemeRole::Accent),
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
                        selected_style(theme, true)
                    } else {
                        theme.style(UiThemeRole::Input)
                    },
                ));
            }
            if targets.is_empty() {
                lines.push(Line::styled(
                    "No reachable recipients yet.",
                    theme.style(UiThemeRole::Attention),
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
                "↑/↓ or j/k select · Enter compose · Esc cancel"
            }));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::bordered().title(" Direct message "))
                    .wrap(Wrap { trim: false }),
                area,
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
                UiMailboxAction::Reply { .. }
                | UiMailboxAction::Direct { .. }
                | UiMailboxAction::Project { .. } => (
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
        UiMailboxDraftTarget::Project {
            thread_id: Some(_), ..
        } => "Continue project conversation",
        UiMailboxDraftTarget::Project {
            thread_id: None, ..
        } => "New project conversation",
    }
}

fn render_too_small(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, area: Rect) {
    let message = vec![
        Line::styled("HQ", theme.style(UiThemeRole::Heading)),
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
            .block(
                Block::bordered()
                    .title(" HQ ")
                    .border_style(theme.style(UiThemeRole::BorderFocused)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_header(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, area: Rect) {
    let status = if model.refreshing() {
        "Updating…"
    } else {
        workspace_connection_label(model.connection())
    };
    let title = Line::from(vec![
        Span::styled(" HQ ", theme.style(UiThemeRole::HeaderBadge)),
        Span::raw("  "),
        Span::styled(
            section_label(model.section()),
            theme.style(UiThemeRole::Heading),
        ),
    ]);
    let context = Line::from(vec![
        Span::styled(" this device ", theme.style(UiThemeRole::TextMuted)),
        Span::styled(status, connection_style(theme, model.connection())),
    ]);
    frame.render_widget(
        Paragraph::new(vec![title, context]).block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(theme.style(UiThemeRole::BorderUnfocused)),
        ),
        area,
    );
}

fn render_wide_content(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    cache: &mut UiRenderCache,
) {
    let [navigation, rows] =
        Layout::horizontal([Constraint::Length(navigation_width()), Constraint::Min(1)])
            .areas(area);
    let navigation_lines = UiSection::ALL
        .into_iter()
        .map(|section| {
            let selected = section == model.section();
            let marker = if selected {
                NAVIGATION_SELECTED_MARKER
            } else {
                NAVIGATION_UNSELECTED_MARKER
            };
            let style = if selected {
                selected_style(theme, model.focus() == UiFocus::Navigation)
            } else {
                theme.style(UiThemeRole::TextMuted)
            };
            Line::styled(format!("{marker}{}", section_label(section)), style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(navigation_lines).block(Block::new().borders(Borders::RIGHT).border_style(
            theme.style(if model.focus() == UiFocus::Navigation {
                UiThemeRole::BorderFocused
            } else {
                UiThemeRole::BorderUnfocused
            }),
        )),
        navigation,
    );
    render_rows(frame, model, theme, rows, cache);
}

fn render_compact_content(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    cache: &mut UiRenderCache,
) {
    let [tabs, rows] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);
    let tab_line = UiSection::ALL
        .into_iter()
        .enumerate()
        .flat_map(|(index, section)| {
            let separator = (index > 0).then(|| Span::raw(" · "));
            separator.into_iter().chain(std::iter::once(Span::styled(
                section_label(section),
                if section == model.section() {
                    selected_style(theme, model.focus() == UiFocus::Navigation)
                } else {
                    theme.style(UiThemeRole::TextMuted)
                },
            )))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(tab_line)).block(
            Block::new()
                .borders(Borders::BOTTOM)
                .border_style(theme.style(if model.focus() == UiFocus::Navigation {
                    UiThemeRole::BorderFocused
                } else {
                    UiThemeRole::BorderUnfocused
                })),
        ),
        tabs,
    );
    render_rows(frame, model, theme, rows, cache);
}

fn render_rows(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    cache: &mut UiRenderCache,
) {
    if model.section() == UiSection::Projects {
        render_projects_workspace(frame, model, theme, area);
    } else if model.section() == UiSection::Inbox || model.conversation().is_some() {
        if model.viewport().width >= WIDE_WIDTH {
            let [summaries, conversation] = Layout::horizontal([
                Constraint::Length(inbox_list_width(area.width)),
                Constraint::Min(1),
            ])
            .areas(area);
            render_summary_rows(frame, model, theme, summaries);
            render_inbox_detail(frame, model, theme, conversation, Borders::LEFT, cache);
        } else {
            let conversation_owns_space =
                matches!(model.focus(), UiFocus::Conversation | UiFocus::Draft);
            let constraints = if conversation_owns_space {
                [Constraint::Length(4), Constraint::Min(1)]
            } else {
                [Constraint::Min(1), Constraint::Length(4)]
            };
            let [summaries, conversation] = Layout::vertical(constraints).areas(area);
            if conversation_owns_space {
                render_compact_selected_summary(frame, model, theme, summaries);
            } else {
                render_summary_rows(frame, model, theme, summaries);
            }
            render_inbox_detail(frame, model, theme, conversation, Borders::TOP, cache);
        }
    } else {
        render_summary_rows(frame, model, theme, area);
    }
}

fn render_projects_workspace(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, area: Rect) {
    if model.viewport().width >= WIDE_WIDTH {
        let [projects, detail] = Layout::horizontal([
            Constraint::Length(inbox_list_width(area.width)),
            Constraint::Min(1),
        ])
        .areas(area);
        render_summary_rows(frame, model, theme, projects);
        render_project_workspace_detail(frame, model, theme, detail, Borders::LEFT, false);
    } else if model.project_workspace_level() == UiProjectWorkspaceLevel::List
        && !project_has_modeless_interaction(model)
    {
        render_summary_rows(frame, model, theme, area);
    } else {
        render_project_workspace_detail(frame, model, theme, area, Borders::NONE, true);
    }
}

fn render_project_workspace_detail(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    borders: Borders,
    compact: bool,
) {
    if project_has_modeless_interaction(model) {
        render_project_interaction(frame, model, theme, area, false);
        return;
    }
    let Some(summary) = model.project_summary() else {
        frame.render_widget(
            Paragraph::new("Select a project to see its summary.")
                .style(theme.style(UiThemeRole::TextMuted))
                .block(Block::new().borders(borders)),
            area,
        );
        return;
    };
    let title = if compact {
        format!("Projects / {} · h/← Projects", summary.name)
    } else {
        format!("Project · {}", summary.name)
    };
    let mut lines = vec![
        Line::styled(
            title,
            pane_title_style(theme, project_detail_pane_focused(model)),
        ),
        Line::from(project_lifecycle_label(summary.lifecycle)),
        Line::default(),
    ];
    match model.project_workspace_level() {
        UiProjectWorkspaceLevel::Manage => {
            lines.extend(project_management_lines(summary, model, theme));
        }
        UiProjectWorkspaceLevel::Folders => {
            lines.extend(project_folder_lines(summary, model, theme));
        }
        UiProjectWorkspaceLevel::List | UiProjectWorkspaceLevel::Summary => {
            lines.extend(project_summary_lines(summary, model, theme));
        }
    }
    if compact {
        lines.push(Line::default());
        lines.push(Line::from(match model.project_workspace_level() {
            UiProjectWorkspaceLevel::Manage => "h/← Project summary",
            UiProjectWorkspaceLevel::Folders => "Tab choose folder · h/← Manage project",
            UiProjectWorkspaceLevel::List => "",
            UiProjectWorkspaceLevel::Summary => "Enter choose · h/← Projects",
        }));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.style(UiThemeRole::Text))
            .block(Block::new().borders(borders).border_style(theme.style(
                if model.focus() == UiFocus::Content {
                    UiThemeRole::BorderFocused
                } else {
                    UiThemeRole::BorderUnfocused
                },
            )))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn project_has_modeless_interaction(model: &UiModel) -> bool {
    matches!(
        model.project_interaction(),
        Some(
            UiProjectInteraction::ChooseCreation { .. }
                | UiProjectInteraction::Search { .. }
                | UiProjectInteraction::CreateExisting { .. }
                | UiProjectInteraction::CreateWorktree { .. }
                | UiProjectInteraction::AddResource { .. }
                | UiProjectInteraction::ReplaceResource { .. }
                | UiProjectInteraction::Activate { .. }
                | UiProjectInteraction::Handoff { .. }
                | UiProjectInteraction::Outcome { .. }
        )
    )
}

fn project_summary_lines(
    summary: &crate::UiProjectSummary,
    model: &UiModel,
    theme: &UiTheme,
) -> Vec<Line<'static>> {
    let selected = |focus| {
        model.project_workspace_level() == UiProjectWorkspaceLevel::Summary
            && model.project_summary_focus() == Some(focus)
    };
    let conversation_label = if summary.conversations.open == 0 {
        if summary.lifecycle == UiProjectLifecycle::Open {
            "Start conversation".to_owned()
        } else {
            "View conversations in Inbox".to_owned()
        }
    } else if summary.conversations.open == 1 {
        "Continue conversation".to_owned()
    } else {
        format!("Open {} conversations in Inbox", summary.conversations.open)
    };
    let agent_name = summary.assigned_agent.name.as_deref().unwrap_or(
        if summary.assigned_agent.agent_id.is_some() {
            "Unnamed agent"
        } else {
            "No agent assigned"
        },
    );
    let mut lines = vec![project_card_line(
        theme,
        "Conversation",
        &conversation_label,
        selected(UiProjectSummaryFocus::Conversation),
    )];
    lines.push(project_card_line(
        theme,
        "Agent",
        &format!(
            "{agent_name} · {}",
            assigned_agent_status_label(summary.assigned_agent.status)
        ),
        selected(UiProjectSummaryFocus::AssignedAgent),
    ));
    lines.push(project_card_line(
        theme,
        "Folders",
        &summary.folders.len().to_string(),
        selected(UiProjectSummaryFocus::Folders),
    ));
    for folder in summary.folders.iter().take(3) {
        lines.push(Line::from(format!(
            "    {}{} · {} · {}",
            if folder.working_folder {
                "Working · "
            } else {
                ""
            },
            folder.path,
            folder.health,
            folder_ownership_label(folder.ownership)
        )));
    }
    if !summary.recovery.is_empty() {
        let recovery = summary
            .recovery
            .iter()
            .map(|recovery| match recovery {
                UiProjectRecoverySummary::FolderOwnership => "folder ownership",
                UiProjectRecoverySummary::AssignedAgentBlocked => "assigned agent",
                UiProjectRecoverySummary::AssignedAgentConflict => "agent responsibility",
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(project_card_line(
            theme,
            "Needs attention",
            &recovery,
            selected(UiProjectSummaryFocus::Recovery),
        ));
    }
    lines.push(project_card_line(
        theme,
        "Manage project…",
        "folders, agent, lifecycle, and technical details",
        selected(UiProjectSummaryFocus::Manage),
    ));
    lines
}

fn project_management_lines(
    summary: &crate::UiProjectSummary,
    model: &UiModel,
    theme: &UiTheme,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        "Manage project",
        theme.style(UiThemeRole::Accent),
    )];
    let action = model.project_management_action();
    let mut push = |candidate, label: &str| {
        lines.push(project_card_line(
            theme,
            label,
            "",
            action == Some(candidate),
        ));
    };
    push(UiProjectManagementAction::Folders, "Folders…");
    if summary.lifecycle == UiProjectLifecycle::Open {
        push(
            if summary.assigned_agent.agent_id.is_some() {
                UiProjectManagementAction::ChangeAssignedAgent
            } else {
                UiProjectManagementAction::AssignAgent
            },
            if summary.assigned_agent.agent_id.is_some() {
                "Change assigned agent"
            } else {
                "Assign agent"
            },
        );
    }
    match summary.lifecycle {
        UiProjectLifecycle::Open => push(UiProjectManagementAction::CloseProject, "Close project"),
        UiProjectLifecycle::Closed => {
            push(UiProjectManagementAction::ReopenProject, "Reopen project");
        }
        UiProjectLifecycle::Archived => push(
            UiProjectManagementAction::RestoreArchivedProject,
            "Restore archived project",
        ),
        UiProjectLifecycle::Closing | UiProjectLifecycle::NeedsAttention => {}
    }
    if summary.lifecycle != UiProjectLifecycle::Archived {
        push(UiProjectManagementAction::ArchiveProject, "Archive project");
    }
    push(
        UiProjectManagementAction::TechnicalDetails,
        "Technical details",
    );
    lines
}

fn project_folder_lines(
    summary: &crate::UiProjectSummary,
    model: &UiModel,
    theme: &UiTheme,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled("Folders", theme.style(UiThemeRole::Accent))];
    if summary.folders.is_empty() {
        lines.push(Line::from(" No folders are attached."));
    }
    for folder in &summary.folders {
        lines.push(project_card_line(
            theme,
            &folder.path,
            if folder.working_folder {
                "Working folder"
            } else {
                &folder.health
            },
            model.selected_project_folder() == Some(folder.folder_id),
        ));
    }
    lines.push(Line::default());
    if let Some(action) = model.project_folder_action() {
        for candidate in [
            UiProjectFolderAction::AddFolder,
            UiProjectFolderAction::ChangeFolderPath,
            UiProjectFolderAction::RemoveFolder,
            UiProjectFolderAction::UseAsWorkingFolder,
            UiProjectFolderAction::CheckFolderNow,
            UiProjectFolderAction::CheckAllFolders,
        ] {
            let label = match candidate {
                UiProjectFolderAction::AddFolder => "Add folder…",
                UiProjectFolderAction::ChangeFolderPath => "Change folder path…",
                UiProjectFolderAction::RemoveFolder => "Remove folder…",
                UiProjectFolderAction::UseAsWorkingFolder => "Use as working folder",
                UiProjectFolderAction::CheckFolderNow => "Check folder now",
                UiProjectFolderAction::CheckAllFolders => "Check all folders",
            };
            lines.push(project_card_line(theme, label, "", action == candidate));
        }
    }
    lines
}

fn project_card_line(theme: &UiTheme, label: &str, value: &str, selected: bool) -> Line<'static> {
    Line::styled(
        format!(" {}{label} · {value}", if selected { '›' } else { ' ' }),
        if selected {
            selected_style(theme, true)
        } else {
            theme.style(UiThemeRole::Text)
        },
    )
}

const fn project_lifecycle_label(lifecycle: UiProjectLifecycle) -> &'static str {
    match lifecycle {
        UiProjectLifecycle::Open => "Open",
        UiProjectLifecycle::Closing => "Closing",
        UiProjectLifecycle::Closed => "Closed",
        UiProjectLifecycle::Archived => "Archived · Closed",
        UiProjectLifecycle::NeedsAttention => "Needs attention",
    }
}

const fn assigned_agent_status_label(status: UiProjectAssignedAgentStatus) -> &'static str {
    match status {
        UiProjectAssignedAgentStatus::Unassigned => "waiting for assignment",
        UiProjectAssignedAgentStatus::SettingUp => "Setting up",
        UiProjectAssignedAgentStatus::Ready => "Ready",
        UiProjectAssignedAgentStatus::NeedsAttention => "Needs attention",
    }
}

const fn folder_ownership_label(ownership: UiProjectFolderOwnership) -> &'static str {
    match ownership {
        UiProjectFolderOwnership::Owned => "Owned by this project",
        UiProjectFolderOwnership::Conflicted => "Ownership conflict",
        UiProjectFolderOwnership::NeedsAttention => "Ownership needs attention",
    }
}

fn render_compact_selected_summary(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
) {
    let count = model.rows().map_or(0, <[UiRow]>::len);
    let mut lines = vec![
        Line::styled(
            format!(
                "{} · {count} {}",
                section_label(model.section()),
                section_item_label(model.section(), count)
            ),
            pane_title_style(theme, summary_pane_focused(model)),
        ),
        Line::default(),
    ];
    if let Some(row) = model.selected_row_data() {
        lines.extend(render_row(model, row, theme, area.width));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_inbox_detail(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    outer_border: Borders,
    cache: &mut UiRenderCache,
) {
    if model.mailbox_draft().is_some() {
        let draft_height = (area.height / 3).max(6).min(area.height.saturating_sub(2));
        let [conversation, draft] =
            Layout::vertical([Constraint::Min(2), Constraint::Length(draft_height)]).areas(area);
        if model.technical_visible() {
            render_technical_inspector(frame, model, theme, conversation, outer_border);
        } else {
            render_conversation(frame, model, theme, conversation, outer_border, cache);
        }
        render_draft_pane(frame, model, theme, draft, Borders::TOP | outer_border);
    } else if model.technical_visible() && model.viewport().width >= WIDE_WIDTH {
        let inspector_height = (area.height / 3).clamp(7, 12);
        let [conversation, inspector] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(inspector_height.min(area.height.saturating_sub(2))),
        ])
        .areas(area);
        render_conversation(frame, model, theme, conversation, outer_border, cache);
        render_technical_inspector(frame, model, theme, inspector, Borders::TOP | outer_border);
    } else if model.technical_visible() {
        render_technical_inspector(frame, model, theme, area, outer_border);
    } else {
        render_conversation(frame, model, theme, area, outer_border, cache);
    }
}

fn render_draft_pane(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    borders: Borders,
) {
    let Some(draft_pane) = model.mailbox_draft() else {
        return;
    };
    match draft_pane {
        UiMailboxDraftPane::Loading { target } => {
            let block = with_draft_context_title(
                Block::new()
                    .borders(borders)
                    .border_style(theme.style(UiThemeRole::BorderFocused))
                    .title(Line::from(" loading draft ").right_aligned()),
                model,
                theme,
                area.width,
            );
            frame.render_widget(
                Paragraph::new(format!("Loading {} draft…", draft_target_label(target)))
                    .block(block),
                area,
            );
        }
        UiMailboxDraftPane::Editing {
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
            let block = with_draft_context_title(
                Block::new()
                    .borders(borders)
                    .border_style(theme.style(if model.focus() == UiFocus::Draft {
                        UiThemeRole::BorderFocused
                    } else {
                        UiThemeRole::BorderUnfocused
                    }))
                    .title(
                        Line::from(format!(
                            " {status} · {}/{} bytes ",
                            draft.content.len(),
                            MAX_DRAFT_BYTES
                        ))
                        .right_aligned(),
                    ),
                model,
                theme,
                area.width,
            );
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let [text_area, hint] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
            let cursor = model.message_field_cursor(&draft.content);
            let mut content = draft.content.clone();
            if model.focus() == UiFocus::Draft {
                content.insert(cursor, '│');
            }
            let content = inert_draft_source(&content);
            frame.render_widget(
                Paragraph::new(content)
                    .style(theme.style(UiThemeRole::Input))
                    .wrap(Wrap { trim: false }),
                text_area,
            );
            let hint_text = model
                .message_field_error()
                .unwrap_or("Enter send · Esc save and close");
            frame.render_widget(
                Paragraph::new(hint_text).style(if model.message_field_error().is_some() {
                    theme.style(UiThemeRole::Error)
                } else {
                    theme.style(UiThemeRole::Footer)
                }),
                hint,
            );
        }
    }
}

fn with_draft_context_title<'a>(
    block: Block<'a>,
    model: &UiModel,
    theme: &UiTheme,
    width: u16,
) -> Block<'a> {
    let Some(context) =
        draft_context_label(model.draft_recipient_name(), model.draft_project_name())
    else {
        return block;
    };
    let maximum_width = usize::from(width.saturating_sub(4) / 2);
    let context = clipped_preview_line(&context, maximum_width);
    block.title(Line::styled(
        format!("{context} "),
        theme.style(UiThemeRole::ConversationProjectContext),
    ))
}

fn draft_context_label(recipient: Option<&str>, project: Option<&str>) -> Option<String> {
    match (recipient, project) {
        (Some(recipient), Some(project)) => Some(format!("To: {recipient} · {project}")),
        (Some(recipient), None) => Some(format!("To: {recipient}")),
        (None, Some(project)) => Some(project.to_owned()),
        (None, None) => None,
    }
}

fn inert_draft_source(source: &str) -> String {
    let mut safe = String::with_capacity(source.len());
    for character in source.chars() {
        match character {
            '\n' => safe.push('\n'),
            '\t' => safe.push_str("    "),
            character if character.is_control() => safe.push(' '),
            character => safe.push(character),
        }
    }
    safe
}

fn render_summary_rows(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, area: Rect) {
    let count = model.rows().map_or(0, <[UiRow]>::len);
    let mut lines = vec![
        Line::styled(
            format!(
                "{} · {count} {}",
                section_label(model.section()),
                section_item_label(model.section(), count)
            ),
            pane_title_style(theme, summary_pane_focused(model)),
        ),
        Line::default(),
    ];
    if let Some(filter) = model.project_filter() {
        lines.push(Line::styled(
            format!(" Project: {} · Esc clear filter", filter.project_name),
            theme.style(UiThemeRole::Accent),
        ));
        lines.push(Line::default());
    }
    match model.human_state() {
        Some(UiHumanState::NeedsAttention(issue)) => {
            lines.extend(human_issue_lines(issue, theme));
            lines.push(Line::default());
        }
        Some(UiHumanState::Ready) | None => {}
    }
    if let Some(onboarding) = onboarding_lines(model, theme) {
        lines.extend(onboarding);
        lines.push(Line::default());
    }
    match model.rows() {
        Some([]) => lines.extend(empty_section_lines(model.section(), theme)),
        Some(rows) => {
            for row in rows {
                lines.extend(render_row(model, row, theme, area.width));
            }
        }
        None => lines.push(Line::styled(
            " Loading your workspace…",
            theme.style(UiThemeRole::Warning),
        )),
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn onboarding_lines(model: &UiModel, theme: &UiTheme) -> Option<Vec<Line<'static>>> {
    let snapshot = model.snapshot()?;
    if model.section() != UiSection::Inbox
        || !matches!(snapshot.human_state, UiHumanState::Ready)
        || !snapshot.inbox_rows.is_empty()
        || !snapshot.sent_rows.is_empty()
        || !snapshot.archived_rows.is_empty()
    {
        return None;
    }
    let has_project = snapshot.projects.iter().any(|project| !project.archived);
    let has_agent = snapshot
        .agents
        .iter()
        .any(|agent| matches!(agent.lifecycle, crate::UiAgentLifecycle::Active));
    let has_provider = snapshot.providers.iter().any(|provider| provider.available);
    let mut lines = vec![Line::styled(
        " Get started with HQ",
        theme.style(UiThemeRole::Heading),
    )];
    lines.push(Line::from(" ✓ Account ready"));
    if !has_project {
        lines.push(Line::styled(
            " › Current: add a project and choose the folder or resource it owns",
            theme.style(UiThemeRole::Attention),
        ));
        lines.push(Line::from(
            " Press n New… and choose “Work with an agent on a project.”",
        ));
        return Some(lines);
    }
    lines.push(Line::from(" ✓ Project ready"));
    if !has_agent {
        lines.push(Line::styled(
            " › Current: create an agent to do the work",
            theme.style(UiThemeRole::Attention),
        ));
        lines.push(Line::from(
            " Press n New…; the project path can create one in place.",
        ));
        return Some(lines);
    }
    lines.push(Line::from(" ✓ Agent ready"));
    if !has_provider {
        lines.push(Line::styled(
            " › Current: connect an agent service",
            theme.style(UiThemeRole::Attention),
        ));
        lines.push(Line::from(
            " Install or enable an agent service, then restart HQ to detect it.",
        ));
        return Some(lines);
    }
    lines.push(Line::from(" ✓ Agent service ready"));
    lines.push(Line::styled(
        " › Current: open the first project conversation",
        theme.style(UiThemeRole::Attention),
    ));
    lines.push(Line::from(
        " Press n New…. HQ will ask you to choose only if more than one service is available, then open the Inbox draft.",
    ));
    Some(lines)
}

fn human_issue_lines(issue: &UiHumanIssue, theme: &UiTheme) -> Vec<Line<'static>> {
    let heading = theme.style(UiThemeRole::Attention);
    match issue {
        UiHumanIssue::NoAccountSelected => vec![
            Line::styled(" No human account is selected on this device.", heading),
            Line::from(" Your account holds your conversations and collaboration identity."),
            Line::from(" Create one now: hq human create"),
            Line::from(" Keep this screen open; when setup finishes, press F5 to continue."),
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

fn empty_section_lines(section: UiSection, theme: &UiTheme) -> Vec<Line<'static>> {
    let heading = theme.style(UiThemeRole::Heading);
    match section {
        UiSection::Inbox => vec![
            Line::styled(" No conversations need your attention.", heading),
            Line::from(" Press n New… for project work, a message, or a personal note."),
        ],
        UiSection::Sent => vec![
            Line::styled(
                " You have not started or replied to a conversation.",
                heading,
            ),
            Line::from(" Press n New… for project work, a message, or a personal note."),
        ],
        UiSection::Archived => vec![
            Line::styled(" You have not put any conversations away.", heading),
            Line::from(" Browse Inbox or Sent to find an active conversation."),
        ],
        UiSection::Agents => vec![
            Line::styled(" No named workers yet.", heading),
            Line::from(" Press n New… to start guided work, or c to create an agent."),
        ],
        UiSection::Projects => vec![
            Line::styled(" No projects yet.", heading),
            Line::from(" A project records work and ownership of its folders and resources."),
            Line::from(" Press n New… to start guided work, or c to create a project."),
        ],
    }
}

fn render_conversation(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    divider: Borders,
    cache: &mut UiRenderCache,
) {
    let conversation = model.conversation();
    let block =
        Block::new()
            .borders(divider)
            .border_style(if model.focus() == UiFocus::Conversation {
                theme.style(UiThemeRole::BorderFocused)
            } else {
                theme.style(UiThemeRole::BorderUnfocused)
            });
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(conversation) = conversation {
        let mut header = vec![Line::styled(
            conversation.title.as_str(),
            pane_title_style(theme, model.focus() == UiFocus::Conversation),
        )];
        if let Some(context) = &conversation.context {
            header.push(Line::styled(
                context.as_str(),
                theme.style(UiThemeRole::TextMuted),
            ));
        }
        if let Some(interaction) = model.current_interaction() {
            header.push(Line::styled(
                format!("● {} needs an answer", interaction.agent_name),
                theme.style(UiThemeRole::Attention),
            ));
        }
        if model.conversation_older_loading() {
            header.push(Line::styled(
                "Loading older messages…",
                theme.style(UiThemeRole::Warning),
            ));
        } else if model.conversation_failure_is_older() {
            header.push(Line::styled(
                "Older messages could not be loaded · PageDown retry",
                theme.style(UiThemeRole::Attention),
            ));
        } else if conversation.next_cursor.is_some() {
            header.push(Line::styled(
                "Older messages available · PageDown",
                theme.style(UiThemeRole::TextMuted),
            ));
        }
        header.push(Line::default());
        let header_height = u16::try_from(header.len())
            .unwrap_or(u16::MAX)
            .min(inner.height);
        let [header_area, entries_area] =
            Layout::vertical([Constraint::Length(header_height), Constraint::Min(0)]).areas(inner);
        frame.render_widget(Paragraph::new(header), header_area);
        if conversation.entries.is_empty() {
            frame.render_widget(
                Paragraph::new("No messages yet.").style(theme.style(UiThemeRole::TextMuted)),
                entries_area,
            );
        } else {
            render_conversation_entries(frame, model, theme, entries_area, conversation, cache);
        }
    } else if let Some(row) = model
        .selected_row_data()
        .filter(|row| row.kind != UiRowKind::Conversation)
    {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(row.title.clone(), theme.style(UiThemeRole::Heading)),
                Line::from(row.detail.clone()),
            ])
            .wrap(Wrap { trim: false }),
            inner,
        );
    } else if model.selected_row_data().is_some() {
        frame.render_widget(
            Paragraph::new("Select a conversation to view its messages.")
                .style(theme.style(UiThemeRole::TextMuted)),
            inner,
        );
    } else {
        frame.render_widget(
            Paragraph::new("No conversation is selected.")
                .style(theme.style(UiThemeRole::TextMuted)),
            inner,
        );
    }
}

fn render_technical_inspector(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    borders: Borders,
) {
    let block =
        Block::new()
            .borders(borders)
            .border_style(theme.style(if model.technical_visible() {
                UiThemeRole::BorderFocused
            } else {
                UiThemeRole::BorderUnfocused
            }));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(entry) = model.conversation().and_then(|conversation| {
        let anchor = model.conversation_anchor()?;
        conversation.entries.iter().find(|entry| entry.id == anchor)
    }) else {
        frame.render_widget(
            Paragraph::new("No message details are available.")
                .style(theme.style(UiThemeRole::TextMuted)),
            inner,
        );
        return;
    };

    let heading = match entry.presentation {
        UiConversationEntryPresentation::Message { .. } => "Message details",
        UiConversationEntryPresentation::Activity { .. } => "Activity details",
    };
    let mut lines = vec![Line::styled(heading, theme.style(UiThemeRole::Heading))];
    if let UiConversationEntryPresentation::Activity {
        detail, completed, ..
    } = &entry.presentation
    {
        lines.push(Line::from(detail.clone()));
        if let Some(completed) = completed {
            lines.extend(
                completed_item_detail_lines(completed)
                    .into_iter()
                    .map(Line::from),
            );
        }
    }
    for section in &entry.technical {
        append_technical_section(&mut lines, section, theme);
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        "j/k scroll · t/h/← close details · ? help",
        theme.style(UiThemeRole::TextMuted),
    ));
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let maximum_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(usize::from(inner.height));
    let scroll = usize::from(model.technical_scroll()).min(maximum_scroll);
    let scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(paragraph.scroll((scroll, 0)), inner);
}

#[allow(clippy::too_many_lines)]
fn append_technical_section(
    lines: &mut Vec<Line<'static>>,
    section: &UiTechnicalSection,
    theme: &UiTheme,
) {
    let section_style = theme.style(UiThemeRole::Accent);
    match section {
        UiTechnicalSection::Routing { sender, recipient } => {
            lines.push(Line::styled("Routing", section_style));
            lines.push(Line::from(format!("sender: {sender}")));
            lines.push(Line::from(format!(
                "recipient: {}",
                recipient.as_deref().unwrap_or("account")
            )));
        }
        UiTechnicalSection::Semantics {
            purpose,
            presentation,
            provider,
            session,
            operation,
            project,
        } => {
            lines.push(Line::styled("Semantics", section_style));
            lines.push(Line::from(format!("purpose: {purpose}")));
            lines.push(Line::from(format!("presentation: {presentation}")));
            lines.push(Line::from(format!(
                "provider: {}",
                provider.as_deref().unwrap_or("none")
            )));
            lines.push(Line::from(format!(
                "session: {}",
                session.as_deref().unwrap_or("none")
            )));
            lines.push(Line::from(format!(
                "operation: {}",
                operation.as_deref().unwrap_or("none")
            )));
            lines.push(Line::from(format!(
                "project: {}",
                project.as_deref().unwrap_or("none")
            )));
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
        } => {
            lines.push(Line::styled("Evidence", section_style));
            lines.push(Line::from(format!("message: {message_id}")));
            lines.push(Line::from(format!("thread: {thread_id}")));
            lines.push(Line::from(format!(
                "state frontier: {}",
                technical_values(state_frontier)
            )));
            lines.push(Line::from(format!(
                "received by: {}",
                technical_values(peer_received_by)
            )));
            lines.push(Line::from(format!(
                "root fact: {}",
                root_fact.as_deref().unwrap_or("none")
            )));
            lines.push(Line::from(format!(
                "root message: {}",
                root_message.as_deref().unwrap_or("none")
            )));
            lines.push(Line::from(format!("ready answer: {ready_answer}")));
            lines.push(Line::from(format!("thread cancelled: {thread_cancelled}")));
        }
        UiTechnicalSection::Activity {
            sequence,
            source_installation,
            source_mailbox,
            provider,
            session,
            operation,
            item,
            logical_key,
            runtime,
            occurred_at_unix_ms,
            status,
            truncated,
        } => {
            lines.push(Line::styled("Activity", section_style));
            lines.push(Line::from(format!("sequence: {sequence}")));
            lines.push(Line::from(format!(
                "source installation: {source_installation}"
            )));
            lines.push(Line::from(format!("source mailbox: {source_mailbox}")));
            lines.push(Line::from(format!("provider: {provider}")));
            lines.push(Line::from(format!("session: {session}")));
            lines.push(Line::from(format!("operation: {operation}")));
            lines.push(Line::from(format!(
                "item: {}",
                item.as_deref().unwrap_or("none")
            )));
            lines.push(Line::from(format!("logical key: {logical_key}")));
            lines.push(Line::from(format!("runtime: {runtime}")));
            lines.push(Line::from(format!(
                "occurred at (unix ms): {occurred_at_unix_ms}"
            )));
            lines.push(Line::from(format!(
                "status: {}",
                activity_status_label(status)
            )));
            lines.push(Line::from(format!("truncated: {truncated}")));
        }
    }
}

fn technical_values(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

fn render_conversation_entries(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    conversation: &crate::UiConversation,
    cache: &mut UiRenderCache,
) {
    if area.width == 0 || area.height == 0 || conversation.entries.is_empty() {
        return;
    }
    let visible_entries = if model.current_interaction().is_some_and(|interaction| {
        conversation
            .entries
            .last()
            .is_some_and(|entry| interaction_supersedes_live_tail(interaction, entry))
    }) {
        &conversation.entries[..conversation.entries.len() - 1]
    } else {
        &conversation.entries
    };
    if visible_entries.is_empty() {
        return;
    }
    let layouts = visible_entries
        .iter()
        .map(|entry| conversation_entry_layout(entry, area.width, theme, cache))
        .collect::<Vec<_>>();
    let heights = layouts
        .iter()
        .map(|layout| layout.height)
        .collect::<Vec<_>>();
    cache.conversation_viewport = Some(UiConversationViewportObservation {
        conversation_id: conversation.row_id.clone(),
        width: area.width,
        height: area.height,
        entries: visible_entries
            .iter()
            .zip(&heights)
            .map(|(entry, height)| UiConversationEntryGeometry {
                entry_id: entry.id.clone(),
                height: *height,
            })
            .collect(),
    });
    let selected_index = model
        .conversation_anchor()
        .and_then(|anchor| visible_entries.iter().position(|entry| entry.id == anchor))
        .unwrap_or(0);
    let visible = visible_conversation_entries(&heights, selected_index, area.height);
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    for index in visible {
        if y >= bottom {
            break;
        }
        let entry = &visible_entries[index];
        let layout = &layouts[index];
        let height = layout.height.min(bottom - y);
        render_conversation_entry(
            frame,
            model,
            entry,
            layout,
            theme,
            Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            },
        );
        y = y.saturating_add(height);
    }
}

fn interaction_supersedes_live_tail(
    interaction: &crate::UiInteraction,
    entry: &UiConversationEntry,
) -> bool {
    if !matches!(
        &entry.presentation,
        UiConversationEntryPresentation::Activity {
            kind: UiConversationActivityKind::AgentTurn | UiConversationActivityKind::Progress,
            status: UiActivityStatus::Running,
            ..
        }
    ) {
        return false;
    }
    let operation = full_identity(interaction.operation_id);
    entry.technical.iter().any(|section| {
        matches!(
            section,
            UiTechnicalSection::Activity {
                provider,
                session,
                operation: candidate,
                ..
            } if provider == &interaction.provider
                && session == &interaction.session
                && candidate == &operation
        )
    })
}

struct ConversationEntryLayout {
    message: Option<Arc<RenderedMessage>>,
    activity: Option<Vec<String>>,
    height: u16,
}

fn conversation_entry_layout(
    entry: &UiConversationEntry,
    width: u16,
    theme: &UiTheme,
    cache: &mut UiRenderCache,
) -> ConversationEntryLayout {
    match &entry.presentation {
        UiConversationEntryPresentation::Message { body, .. } => {
            let message = cache.messages.render(&entry.id, body, width, theme);
            let height = message.body_height().saturating_add(2);
            ConversationEntryLayout {
                message: Some(message),
                activity: None,
                height,
            }
        }
        UiConversationEntryPresentation::Activity { .. } => {
            let activity = activity_preview(entry, width);
            let height = u16::try_from(activity.len())
                .unwrap_or(u16::MAX)
                .saturating_add(1);
            ConversationEntryLayout {
                message: None,
                activity: Some(activity),
                height,
            }
        }
    }
}

fn activity_preview(entry: &UiConversationEntry, width: u16) -> Vec<String> {
    const PREVIEW_LINES: usize = 3;
    let UiConversationEntryPresentation::Activity {
        summary,
        status,
        completed,
        ..
    } = &entry.presentation
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    match completed {
        Some(UiCompletedItemPresentation::Command {
            command,
            output,
            exit_code,
            command_truncated,
            output_truncated,
        }) => {
            let state = match status {
                UiActivityStatus::Succeeded => "completed",
                UiActivityStatus::Failed { .. } => "failed",
                UiActivityStatus::Interrupted => "interrupted",
                UiActivityStatus::Snapshot | UiActivityStatus::Running => "activity",
            };
            let exit = exit_code.map_or_else(String::new, |code| format!(" · exit {code}"));
            lines.push(format!("Command {state}{exit}"));
            let command_lines = command.lines().collect::<Vec<_>>();
            for (index, line) in command_lines.iter().take(PREVIEW_LINES).enumerate() {
                lines.push(format!("{}{}", if index == 0 { "$ " } else { "  " }, line));
            }
            if *command_truncated || command_lines.len() > PREVIEW_LINES {
                lines.push(format!(
                    "  {}",
                    preview_omission(
                        command_lines.len().saturating_sub(PREVIEW_LINES),
                        *command_truncated,
                        "command"
                    )
                ));
            }
            if let Some(output) = output {
                let output_lines = output.lines().collect::<Vec<_>>();
                lines.extend(
                    output_lines
                        .iter()
                        .take(PREVIEW_LINES)
                        .map(|line| format!("│ {line}")),
                );
                if *output_truncated || output_lines.len() > PREVIEW_LINES {
                    lines.push(format!(
                        "│ {}",
                        preview_omission(
                            output_lines.len().saturating_sub(PREVIEW_LINES),
                            *output_truncated,
                            "output"
                        )
                    ));
                }
            }
        }
        Some(UiCompletedItemPresentation::FileChange {
            changes,
            changes_truncated,
        }) => {
            lines.push(summary.clone());
            lines.extend(
                changes
                    .iter()
                    .take(PREVIEW_LINES)
                    .map(|change| format!("  {}", change.path)),
            );
            if *changes_truncated || changes.len() > PREVIEW_LINES {
                lines.push(format!(
                    "  … {} more",
                    changes.len().saturating_sub(PREVIEW_LINES)
                ));
            }
        }
        Some(
            UiCompletedItemPresentation::Tool { .. }
            | UiCompletedItemPresentation::WebSearch { .. }
            | UiCompletedItemPresentation::Unknown,
        )
        | None => lines.push(summary.clone()),
    }
    lines
        .into_iter()
        .map(|line| clipped_preview_line(&line, usize::from(width)))
        .collect()
}

fn preview_omission(known_lines: usize, source_truncated: bool, subject: &str) -> String {
    if known_lines == 0 {
        return format!("… more {subject} omitted (t to view retained {subject})");
    }
    let noun = if known_lines == 1 { "line" } else { "lines" };
    if source_truncated {
        format!("… +{known_lines} or more {noun} (t to view retained {subject})")
    } else {
        format!("… +{known_lines} {noun} (t to view full {subject})")
    }
}

fn completed_item_detail_lines(completed: &UiCompletedItemPresentation) -> Vec<String> {
    let UiCompletedItemPresentation::Command {
        command,
        output,
        exit_code,
        command_truncated,
        output_truncated,
    } = completed
    else {
        return Vec::new();
    };
    let mut lines = vec!["Command".to_owned()];
    lines.extend(
        command
            .lines()
            .enumerate()
            .map(|(index, line)| format!("{}{}", if index == 0 { "$ " } else { "  " }, line)),
    );
    if *command_truncated {
        lines.push("… command truncated at the provider boundary".to_owned());
    }
    lines.push("Output".to_owned());
    if let Some(output) = output {
        if output.is_empty() {
            lines.push("(no output)".to_owned());
        } else {
            lines.extend(output.lines().map(|line| format!("│ {line}")));
        }
    } else {
        lines.push("(no output)".to_owned());
    }
    if *output_truncated {
        lines.push("… output truncated at the provider boundary".to_owned());
    }
    lines.push(exit_code.map_or_else(
        || "Exit code: unavailable".to_owned(),
        |code| format!("Exit code: {code}"),
    ));
    lines
}

fn clipped_preview_line(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    format!("{}…", display_prefix(value, width.saturating_sub(1)))
}

fn visible_conversation_entries(
    heights: &[u16],
    selected_index: usize,
    available_height: u16,
) -> std::ops::Range<usize> {
    if heights.is_empty() || available_height == 0 {
        return 0..0;
    }
    let selected_index = selected_index.min(heights.len() - 1);
    let mut start = selected_index;
    let mut end = selected_index + 1;
    let mut used = heights[selected_index].min(available_height);
    let mut above_budget = available_height.saturating_sub(used) / 2;
    while start > 0 && heights[start - 1] <= above_budget {
        start -= 1;
        used = used.saturating_add(heights[start]);
        above_budget = above_budget.saturating_sub(heights[start]);
    }
    while end < heights.len() && heights[end] <= available_height.saturating_sub(used) {
        used = used.saturating_add(heights[end]);
        end += 1;
    }
    while start > 0 && heights[start - 1] <= available_height.saturating_sub(used) {
        start -= 1;
        used = used.saturating_add(heights[start]);
    }
    start..end
}

fn render_conversation_entry(
    frame: &mut Frame<'_>,
    model: &UiModel,
    entry: &UiConversationEntry,
    layout: &ConversationEntryLayout,
    theme: &UiTheme,
    area: Rect,
) {
    if area.is_empty() {
        return;
    }
    let selected = model.conversation_anchor() == Some(entry.id.as_str());
    if selected && matches!(model.focus(), UiFocus::Conversation | UiFocus::Draft) {
        let role = if model.focus() == UiFocus::Conversation && !model.technical_visible() {
            UiThemeRole::ConversationSelectionFocused
        } else {
            UiThemeRole::ConversationSelectionUnfocused
        };
        frame.render_widget(Block::new().style(theme.style(role)), area);
    }
    match &entry.presentation {
        UiConversationEntryPresentation::Message { author, .. } => {
            let (author, author_role) = match author {
                UiConversationAuthor::You => ("You", UiThemeRole::ConversationAuthorSelf),
                UiConversationAuthor::Participant(label) => {
                    (label.as_str(), UiThemeRole::ConversationAuthorParticipant)
                }
                UiConversationAuthor::Unknown => {
                    ("Unknown sender", UiThemeRole::ConversationAuthorParticipant)
                }
            };
            let delivery = if entry.message_state == Some(UiMessageState::Rejected) {
                ""
            } else {
                match entry.delivery {
                    Some(UiMessageDelivery::Pending) => " · Pending",
                    Some(UiMessageDelivery::Sent) | None => "",
                    Some(UiMessageDelivery::Received) => " · Received",
                }
            };
            let exceptional = match entry.message_state {
                Some(UiMessageState::Archived) => " · Archived",
                Some(UiMessageState::Rejected) => " · Could not be delivered",
                Some(UiMessageState::Open) | None => "",
            };
            frame.render_widget(
                Paragraph::new(format!("{author}{delivery}{exceptional}"))
                    .style(theme.style(author_role)),
                Rect { height: 1, ..area },
            );
            if area.height > 1
                && let Some(message) = &layout.message
            {
                frame.render_widget(
                    Paragraph::new(message.text().clone()).style(theme.style(UiThemeRole::Text)),
                    Rect {
                        y: area.y + 1,
                        height: area.height.saturating_sub(2),
                        ..area
                    },
                );
            }
        }
        UiConversationEntryPresentation::Activity { status, .. } => {
            let symbol = match status {
                UiActivityStatus::Snapshot => "·",
                UiActivityStatus::Running => "●",
                UiActivityStatus::Succeeded => "✓",
                UiActivityStatus::Failed { .. } => "!",
                UiActivityStatus::Interrupted => "×",
            };
            let role = match status {
                UiActivityStatus::Snapshot | UiActivityStatus::Running => {
                    UiThemeRole::ConversationActivity
                }
                UiActivityStatus::Succeeded => UiThemeRole::ConversationActivitySuccess,
                UiActivityStatus::Failed { .. } => UiThemeRole::ConversationActivityError,
                UiActivityStatus::Interrupted => UiThemeRole::ConversationActivityWarning,
            };
            if let Some(lines) = &layout.activity {
                let mut rendered = lines
                    .iter()
                    .map(|line| Line::from(line.clone()))
                    .collect::<Vec<_>>();
                if let Some(first) = rendered.first_mut() {
                    first.spans.insert(0, Span::raw(format!("{symbol} ")));
                }
                frame.render_widget(
                    Paragraph::new(rendered).style(theme.style(role)),
                    Rect {
                        height: area.height.saturating_sub(1),
                        ..area
                    },
                );
            }
        }
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

fn render_row<'row>(
    model: &UiModel,
    row: &'row UiRow,
    theme: &UiTheme,
    width: u16,
) -> [Line<'row>; 2] {
    let selected = model.selected_row() == Some(row.id.as_str());
    if row.kind == UiRowKind::Conversation {
        let title_style = if selected {
            selected_style(theme, model.focus() == UiFocus::Content)
        } else {
            theme.style(UiThemeRole::Text)
        };
        let detail_style = if selected {
            title_style
        } else {
            theme.style(UiThemeRole::TextMuted)
        };
        return [
            Line::styled(padded_display_text(&row.title, width), title_style),
            Line::styled(padded_display_text(&row.detail, width), detail_style),
        ];
    }
    let marker = if selected { " › " } else { "   " };
    let title_style = if selected {
        selected_style(theme, model.focus() == UiFocus::Content)
    } else {
        theme.style(UiThemeRole::Text)
    };
    let detail = if row.kind == UiRowKind::Agent {
        Line::from(vec![
            Span::raw("     "),
            Span::styled(row.detail.as_str(), row_state_style(theme, row.state)),
        ])
    } else {
        Line::from(vec![
            Span::raw("     "),
            Span::styled(
                row_state_label(model.section(), row.state),
                row_state_style(theme, row.state),
            ),
            Span::raw(" · "),
            Span::styled(row.detail.as_str(), theme.style(UiThemeRole::TextMuted)),
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

fn padded_display_text(value: &str, width: u16) -> String {
    let width = usize::from(width);
    let mut visible = display_prefix(value, width).to_owned();
    let padding = width.saturating_sub(UnicodeWidthStr::width(visible.as_str()));
    visible.extend(std::iter::repeat_n(' ', padding));
    visible
}

fn render_footer(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, area: Rect) {
    let content = if let Some(page) = model.help_page() {
        match page {
            UiHelpPage::Context => " t technical details · F1/?/Esc close help".to_owned(),
            UiHelpPage::Technical => " t contextual help · F1/?/Esc close help".to_owned(),
        }
    } else if model.new_modal().is_some()
        || model.mailbox_modal().is_some()
        || model.agent_modal().is_some()
        || model.project_interaction().is_some()
    {
        " F1 help · Esc back/cancel · q quit".to_owned()
    } else if let Some(failure) = model.last_failure() {
        format!(
            " Could not complete that action · {} · ? details",
            failure.action
        )
    } else if let Some(notice) = model.completion_notice() {
        format!(" Done · {notice}")
    } else if let Some(hint) = model.transient_help() {
        format!(" Hint · {hint}")
    } else if model.focus() == UiFocus::Draft {
        " Enter send · Esc save and close · ? help · q quit".to_owned()
    } else if model.focus() == UiFocus::Navigation && model.viewport().width >= WIDE_WIDTH {
        " ↑/↓ or j/k section · Enter content · ? help · q quit".to_owned()
    } else if model.focus() == UiFocus::Navigation {
        " ←/→ section · Enter content · ? help · q quit".to_owned()
    } else if model.section() == UiSection::Projects {
        match model.project_workspace_level() {
            UiProjectWorkspaceLevel::List if model.selected_row_data().is_some() => {
                " Enter collaborate · l/→ summary · c create · / search · ? help · q quit"
                    .to_owned()
            }
            UiProjectWorkspaceLevel::List => " c create · / search · ? help · q quit".to_owned(),
            UiProjectWorkspaceLevel::Summary => {
                " j/k choose · Enter open · h/← Projects · ? help · q quit".to_owned()
            }
            UiProjectWorkspaceLevel::Manage => {
                " j/k choose action · Enter open · h/← summary · ? help · q quit".to_owned()
            }
            UiProjectWorkspaceLevel::Folders => {
                " j/k choose action · Tab folder · Enter run · h/← manage · ? help · q quit"
                    .to_owned()
            }
        }
    } else if model.section() == UiSection::Agents {
        if model.selected_row_data().is_some() {
            " Enter inspect · n New… · c create · / search · ? help · q quit".to_owned()
        } else {
            " n New… · c create · / search · ? help · q quit".to_owned()
        }
    } else if model.focus() == UiFocus::Conversation {
        conversation_footer(model)
    } else if model.selected_row_data().is_none() {
        " n New… · d message · N note · ? help · q quit".to_owned()
    } else if model.viewport().width >= WIDE_WIDTH {
        " Enter open · n New… · d message · N note · ? help · q quit".to_owned()
    } else {
        " Enter open · ? help · q quit".to_owned()
    };
    let style = if model.last_failure().is_some() && model.help_page().is_none() {
        theme.style(UiThemeRole::FooterWarning)
    } else if model.completion_notice().is_some() && model.help_page().is_none() {
        theme.style(UiThemeRole::FooterSuccess)
    } else {
        theme.style(UiThemeRole::Footer)
    };
    frame.render_widget(
        Paragraph::new(Line::styled(content, style)).block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(theme.style(UiThemeRole::BorderUnfocused)),
        ),
        area,
    );
}

fn conversation_footer(model: &UiModel) -> String {
    let selected = model.conversation().and_then(|conversation| {
        let anchor = model.conversation_anchor()?;
        conversation.entries.iter().find(|entry| entry.id == anchor)
    });
    if model.technical_visible() {
        return " j/k scroll · t/h/← close details · ? help".to_owned();
    }
    let mut controls = vec![if model.viewport().width >= WIDE_WIDTH {
        "↑/↓ or j/k message"
    } else {
        "j/k"
    }];
    if matches!(
        model
            .selected_row_data()
            .and_then(|row| row.conversation_target),
        Some(crate::model::UiConversationTarget::Project { .. })
    ) {
        controls.push("r continue");
        controls.push("c new conversation");
    } else if selected
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
        "t/Enter details"
    } else {
        "t/Enter info"
    });
    controls.push("h/← Inbox");
    controls.push("? help");
    format!(" {}", controls.join(" · "))
}

fn selected_style(theme: &UiTheme, focused: bool) -> Style {
    theme.style(if focused {
        UiThemeRole::SelectionFocused
    } else {
        UiThemeRole::SelectionUnfocused
    })
}

fn pane_title_style(theme: &UiTheme, focused: bool) -> Style {
    theme.style(if focused {
        UiThemeRole::PaneTitleFocused
    } else {
        UiThemeRole::PaneTitleUnfocused
    })
}

fn summary_pane_focused(model: &UiModel) -> bool {
    model.focus() == UiFocus::Content
        && (model.section() != UiSection::Projects
            || model.project_workspace_level() == UiProjectWorkspaceLevel::List)
}

fn project_detail_pane_focused(model: &UiModel) -> bool {
    model.focus() == UiFocus::Content
        && model.project_workspace_level() != UiProjectWorkspaceLevel::List
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

fn connection_style(theme: &UiTheme, state: UiConnectionState) -> Style {
    theme.style(match state {
        UiConnectionState::Ready => UiThemeRole::ConnectionReady,
        UiConnectionState::Connecting | UiConnectionState::Reconnecting => {
            UiThemeRole::ConnectionPending
        }
        UiConnectionState::Disconnected | UiConnectionState::Incompatible => {
            UiThemeRole::ConnectionError
        }
    })
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

fn row_state_style(theme: &UiTheme, state: UiRowState) -> Style {
    theme.style(match state {
        UiRowState::Open => UiThemeRole::RowOpen,
        UiRowState::Waiting => UiThemeRole::RowWaiting,
        UiRowState::Archived => UiThemeRole::RowArchived,
        UiRowState::Attention => UiThemeRole::RowAttention,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        activity_preview, completed_item_detail_lines, conversation_entry_layout, display_prefix,
        display_suffix, draft_context_label, inbox_list_width, inert_draft_source,
        interaction_supersedes_live_tail, navigation_width, render_conversation_entries,
        text_field_line, visible_conversation_entries,
    };
    use crate::{
        UiActivityStatus, UiCompletedItemPresentation, UiConversationActivityKind,
        UiConversationAuthor, UiConversationEntry, UiConversationEntryPresentation,
        UiConversationViewportObservation, UiInteraction, UiInteractionKind, UiMessageState,
        UiModel, UiSize, UiTechnicalSection, UiTheme,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn navigation_width_leaves_one_cell_after_the_longest_label() {
        let longest_row = " › Archived";
        let longest_row_width = u16::try_from(longest_row.width()).unwrap_or(u16::MAX);

        assert_eq!(navigation_width(), longest_row_width + 2);
    }

    #[test]
    fn pending_interaction_supersedes_only_its_exact_running_tail() {
        let interaction = UiInteraction {
            agent_id: [1; 32],
            agent_name: "alice".to_owned(),
            project_id: Some([2; 32]),
            project_name: Some("release".to_owned()),
            provider: "codex".to_owned(),
            session: "session-1".to_owned(),
            request_id: [3; 32],
            operation_id: [4; 32],
            kind: UiInteractionKind::CommandApproval,
            prompt: "Run tests?".to_owned(),
            choices: Vec::new(),
            allow_text: false,
        };
        let mut entry = UiConversationEntry {
            id: "tail".to_owned(),
            presentation: UiConversationEntryPresentation::Activity {
                kind: UiConversationActivityKind::Progress,
                status: UiActivityStatus::Running,
                summary: "running tests".to_owned(),
                detail: "running tests".to_owned(),
                truncated: false,
                completed: None,
            },
            message_state: None,
            delivery: None,
            message_target: None,
            technical: vec![UiTechnicalSection::Activity {
                sequence: 1,
                source_installation: "source".to_owned(),
                source_mailbox: "mailbox".to_owned(),
                provider: "codex".to_owned(),
                session: "session-1".to_owned(),
                operation: super::full_identity([4; 32]),
                item: Some("tests".to_owned()),
                logical_key: "tests".to_owned(),
                runtime: "codex".to_owned(),
                occurred_at_unix_ms: 1,
                status: UiActivityStatus::Running,
                truncated: false,
            }],
        };
        assert!(interaction_supersedes_live_tail(&interaction, &entry));

        if let UiConversationEntryPresentation::Activity { status, .. } = &mut entry.presentation {
            *status = UiActivityStatus::Succeeded;
        }
        assert!(!interaction_supersedes_live_tail(&interaction, &entry));
    }

    #[test]
    fn display_clipping_keeps_unicode_scalar_boundaries_and_cell_budgets() {
        assert_eq!(display_prefix("a界b", 3), "a界");
        assert_eq!(display_prefix("a界b", 2), "a");
        assert_eq!(display_suffix("a界b", 3), "界b");
        assert_eq!(display_suffix("a界b", 2), "b");
    }

    #[test]
    fn raw_draft_source_is_terminal_safe_without_changing_markdown() {
        let source = "# raw\n\t**bold**\x1b[31m\u{0085}\u{007f}";
        let rendered = inert_draft_source(source);

        assert_eq!(rendered, "# raw\n    **bold** [31m  ");
        assert!(
            rendered
                .chars()
                .all(|character| !character.is_control() || character == '\n')
        );
    }

    #[test]
    fn draft_context_names_the_recipient_beside_the_project() {
        assert_eq!(
            draft_context_label(Some("alice"), Some("release")),
            Some("To: alice · release".to_owned())
        );
        assert_eq!(
            draft_context_label(Some("alice"), None),
            Some("To: alice".to_owned())
        );
    }

    #[test]
    fn a_wide_character_under_the_caret_cannot_overrun_a_one_cell_field() {
        let line = text_field_line(&UiTheme::terminal(), 10, "Label", "界", 0, true, "");
        let width = line
            .spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>();

        assert_eq!(width, 10);
    }

    #[test]
    fn inbox_list_width_is_bounded_and_gives_growth_to_conversation() {
        assert_eq!(inbox_list_width(72), 24);
        assert_eq!(inbox_list_width(80), 31);
        assert_eq!(inbox_list_width(81), 32);
        assert_eq!(inbox_list_width(120), 32);

        for available in 72..=240 {
            let list = inbox_list_width(available);
            assert!((24..=36).contains(&list));
            if available >= 73 {
                assert!(available - list > 48);
            }
        }
    }

    #[test]
    fn message_height_uses_the_width_specific_artifact_for_words_and_wide_text() {
        let theme = UiTheme::terminal();
        let mut cache = super::UiRenderCache::new();
        assert_eq!(
            conversation_entry_layout(&message("one\ntwo"), 20, &theme, &mut cache).height,
            3
        );
        assert_eq!(
            conversation_entry_layout(&message("abcdefgh"), 3, &theme, &mut cache).height,
            5
        );
        assert_eq!(
            conversation_entry_layout(&message("界界界"), 4, &theme, &mut cache).height,
            4
        );
        assert_eq!(
            conversation_entry_layout(&message(""), 20, &theme, &mut cache).height,
            3
        );
    }

    #[test]
    fn command_preview_is_bounded_to_three_output_lines_with_status_and_omission() {
        let entry = UiConversationEntry {
            id: "command".to_owned(),
            presentation: UiConversationEntryPresentation::Activity {
                kind: UiConversationActivityKind::CompletedItem,
                status: UiActivityStatus::Failed {
                    reason: "command_failed".to_owned(),
                },
                summary: "Command failed".to_owned(),
                detail: "complete detail".to_owned(),
                truncated: false,
                completed: Some(UiCompletedItemPresentation::Command {
                    command: "printf one\nprintf two".to_owned(),
                    output: Some("one\ntwo\nthree\nfour".to_owned()),
                    exit_code: Some(17),
                    command_truncated: false,
                    output_truncated: false,
                }),
            },
            message_state: None,
            delivery: None,
            message_target: None,
            technical: Vec::new(),
        };
        let preview = activity_preview(&entry, 80);
        assert_eq!(preview[0], "Command failed · exit 17");
        assert!(preview.iter().any(|line| line == "$ printf one"));
        assert_eq!(
            preview.iter().filter(|line| line.starts_with("│ ")).count(),
            4
        );
        assert_eq!(
            preview.last().map(String::as_str),
            Some("│ … +1 line (t to view full output)")
        );

        let mut cache = super::UiRenderCache::new();
        assert_eq!(
            conversation_entry_layout(&entry, 80, &UiTheme::terminal(), &mut cache).height,
            u16::try_from(preview.len())
                .unwrap_or(u16::MAX)
                .saturating_add(1)
        );
    }

    #[test]
    fn completed_command_details_include_every_retained_output_line() {
        let completed = UiCompletedItemPresentation::Command {
            command: "printf one\nprintf two".to_owned(),
            output: Some("one\ntwo\nthree\nfour".to_owned()),
            exit_code: Some(17),
            command_truncated: false,
            output_truncated: false,
        };

        assert_eq!(
            completed_item_detail_lines(&completed),
            [
                "Command",
                "$ printf one",
                "  printf two",
                "Output",
                "│ one",
                "│ two",
                "│ three",
                "│ four",
                "Exit code: 17",
            ]
        );
    }

    #[test]
    fn measured_viewport_keeps_the_stable_anchor_visible() {
        let heights = [3, 3, 3, 3, 3];
        assert_eq!(visible_conversation_entries(&heights, 2, 10), 1..4);
        assert_eq!(visible_conversation_entries(&heights, 4, 7), 3..5);
        assert_eq!(visible_conversation_entries(&[12, 3], 0, 5), 0..1);
        assert_eq!(visible_conversation_entries(&heights, 2, 0), 0..0);
    }

    #[test]
    fn renderer_reports_exact_conversation_geometry_to_the_shell() {
        let mut first = message("one");
        first.id = "first".to_owned();
        let mut second = message("two\nthree");
        second.id = "second".to_owned();
        let conversation = crate::UiConversation {
            row_id: "thread-a".to_owned(),
            title: "Alice".to_owned(),
            context: None,
            entries: vec![first, second],
            next_cursor: None,
        };
        let model = UiModel::new(UiSize {
            width: 80,
            height: 24,
        });
        let theme = UiTheme::terminal();
        let mut cache = super::UiRenderCache::new();
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("test terminal");

        terminal
            .draw(|frame| {
                render_conversation_entries(
                    frame,
                    &model,
                    &theme,
                    Rect::new(0, 0, 40, 8),
                    &conversation,
                    &mut cache,
                );
            })
            .expect("render conversation geometry");

        assert_eq!(
            cache.conversation_viewport,
            Some(UiConversationViewportObservation {
                conversation_id: "thread-a".to_owned(),
                width: 40,
                height: 8,
                entries: vec![
                    crate::UiConversationEntryGeometry {
                        entry_id: "first".to_owned(),
                        height: 3,
                    },
                    crate::UiConversationEntryGeometry {
                        entry_id: "second".to_owned(),
                        height: 3,
                    },
                ],
            })
        );
    }

    fn message(body: &str) -> UiConversationEntry {
        UiConversationEntry {
            id: "message".to_owned(),
            presentation: UiConversationEntryPresentation::Message {
                author: UiConversationAuthor::You,
                body: body.to_owned(),
            },
            message_state: Some(UiMessageState::Open),
            delivery: None,
            message_target: None,
            technical: Vec::new(),
        }
    }
}
