//! Borrowed responsive Ratatui renderer.

use std::fmt::Write as _;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    UiActivityStatus, UiAgent, UiAgentAssignmentPhase, UiAgentAttentionReason, UiAgentModal,
    UiAgentProjectAssignment, UiAgentStatus, UiCompletedItemPresentation, UiConfigField,
    UiConnectionState, UiConversationActivityKind, UiConversationAuthor, UiConversationEntry,
    UiConversationEntryGeometry, UiConversationEntryPresentation,
    UiConversationViewportObservation, UiFocus, UiHelpPage, UiHumanIssue,
    UiHumanMembershipEvidence, UiHumanMembershipStatus, UiHumanState, UiInteractionKind,
    UiInteractionModal, UiInteractionTarget, UiInteractionTargetIssue, UiMailboxAction,
    UiMailboxDraftPane, UiMailboxDraftTarget, UiMailboxModal, UiManagedSessionAction,
    UiManagedSessionOutcome, UiMessageDelivery, UiMessageState, UiModel, UiNewChoice, UiNewModal,
    UiProjectAction, UiProjectAssignedAgentStatus, UiProjectCreationChoice, UiProjectFolderAction,
    UiProjectFolderOwnership, UiProjectFormField, UiProjectInteraction, UiProjectLifecycle,
    UiProjectManagementAction, UiProjectOutcome, UiProjectRecoverySummary, UiProjectSummaryFocus,
    UiProjectThread, UiProjectWorkspaceLevel, UiProvider, UiRow, UiRowKind, UiRowState, UiSection,
    UiTechnicalSection, UiTheme, UiThemeRole,
    message_markdown::MessageRenderCache,
    model::WIDE_WIDTH,
    shell_highlight::{ShellHighlightCache, ShellSegment, ShellTokenKind},
};

const MINIMUM_WIDTH: u16 = 40;
const MINIMUM_HEIGHT: u16 = 10;
const INBOX_LIST_MIN_WIDTH: u16 = 24;
const INBOX_LIST_PREFERRED_WIDTH: u16 = 32;
const INBOX_LIST_MAX_WIDTH: u16 = 36;
const CONVERSATION_PREFERRED_MIN_WIDTH: u16 = 48;

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
    commands: ShellHighlightCache,
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
    render_rows(frame, model, theme, content, cache);
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
        UiInteractionModal::Submitting {
            interaction,
            selected,
            text,
        } => (interaction, true, *selected, text.as_str()),
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
                Line::from(format!("Choose or create an agent for {}.", project.name)),
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
        UiNewModal::ChangeSetupAgent {
            setup,
            project,
            agents,
            selected,
        } => {
            let mut lines = vec![
                Line::from(format!("Conversation about {}", project.name)),
                Line::from(format!(
                    "Currently planned for {}. Choose an available agent instead.",
                    setup.agent_name
                )),
                Line::default(),
            ];
            for agent in agents {
                lines.push(project_choice_line(
                    theme,
                    agent.names.first().map_or("Unnamed agent", String::as_str),
                    &agent_status_label(&agent.status),
                    *selected == Some(agent.agent_id),
                ));
            }
            (" Change agent ", lines)
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
    if model.new_modal().is_none()
        && model.project_interaction().is_none()
        && model.agent_modal().is_none()
        && model.mailbox_modal().is_none()
        && model.mailbox_draft().is_some()
    {
        return Some(vec![
            Line::styled(
                "Help for writing a message",
                theme.style(UiThemeRole::Heading),
            ),
            Line::from(
                "HQ saves this draft as you type and retains it if sending needs attention.",
            ),
            Line::default(),
            Line::styled("Editing keys", theme.style(UiThemeRole::Heading)),
            Line::from("←/→ move one character · ↑/↓ move one line"),
            Line::from("Ctrl-A/Home line start · Ctrl-E/End line end"),
            Line::from("Backspace delete left · Delete/Ctrl-D delete right"),
            Line::from("Ctrl-K delete to line end · Ctrl-U delete to line start"),
            Line::from("Ctrl-J/Shift-Enter newline · Enter send · Esc close"),
            Line::default(),
            Line::from("t — technical details · F1 / Esc — close help"),
        ]);
    }
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
            UiNewModal::ChangeSetupAgent { .. } => (
                "Help for changing the planned agent",
                "This replaces the exact agent choice retained with the unsent first message. It does not create a second conversation.",
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
        UiSection::Inbox => "Inbox lists every available agent and their latest conversation.",
        UiSection::Sent => "Sent contains conversations you have started or replied to.",
        UiSection::Archived => "Archived contains conversations you have put away.",
        UiSection::Agents => "Agents are named workers you can assign and contact.",
        UiSection::Projects => "Projects describe work and the resources it owns.",
        UiSection::Config => "Config controls installation-local defaults and appearance.",
    }
}

fn section_help_actions(model: &UiModel) -> Vec<Line<'static>> {
    let mut actions = vec![
        Line::from("1 Inbox · 2 Sent · 3 Archived"),
        Line::from("4 Agents · 5 Projects · 6 Config"),
        Line::from("↑/↓ or j/k — select"),
    ];
    match model.section() {
        UiSection::Inbox | UiSection::Sent | UiSection::Archived => {
            if let Some(setup) = model.conversation_setup() {
                actions.push(Line::from(format!(
                    "Conversation with {} about {} has not started.",
                    setup.agent_name, setup.project_name
                )));
                actions.push(Line::from("r or Enter — write the first message"));
            } else if model.conversation().is_some() {
                actions.push(Line::from(
                    "↑/↓ or j/k — select message · Enter — details · Esc — close conversation",
                ));
                actions.push(Line::from(if model.section() == UiSection::Inbox {
                    "r — reply · d — archive conversation · PgDn — load more"
                } else {
                    "r — reply · PgDn — load more"
                }));
            } else if model.selected_row_data().is_some() {
                actions.push(Line::from("Enter — open selected conversation"));
            }
            actions.push(Line::from(if model.section() == UiSection::Inbox {
                "n — New… · d — archive conversation"
            } else {
                "n — New…"
            }));
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
                    "Enter — collaborate · → — summary · c — create · / — search"
                }
                UiProjectWorkspaceLevel::List => "c — create project · / — search",
                UiProjectWorkspaceLevel::Summary => {
                    "↑/↓ or j/k — choose · Enter — open · ← — Projects"
                }
                UiProjectWorkspaceLevel::Manage => {
                    "↑/↓ or j/k — choose action · ← — project summary"
                }
                UiProjectWorkspaceLevel::Folders => {
                    "↑/↓ or j/k — choose action · Tab — choose folder · ← — manage"
                }
            }));
        }
        UiSection::Config => {
            actions.push(Line::from(
                "↑/↓ or j/k — choose · Enter or → — edit/change · Esc — navigation",
            ));
        }
    }
    actions
}

const fn row_state_technical_label(state: UiRowState) -> &'static str {
    match state {
        UiRowState::Open => "open",
        UiRowState::Unread => "unread",
        UiRowState::Waiting => "waiting",
        UiRowState::Archived => "archived",
        UiRowState::Attention => "needs attention",
    }
}

const fn row_kind_label(kind: UiRowKind) -> &'static str {
    match kind {
        UiRowKind::Conversation => "conversation",
        UiRowKind::ConversationSetup => "conversation setup",
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
                Line::from(if model.guided_project_creation_active() {
                    "↑/↓ or j/k choose · Enter continue · Esc return to project choice"
                } else {
                    "↑/↓ or j/k choose · Enter continue · Esc cancel"
                }),
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
            } else if model.guided_project_recovery_active() {
                "Fields are retained · Enter check the same operation again · Esc recovery details"
            } else if model.guided_project_creation_active() {
                "Tab/Shift-Tab field · Enter create · Esc return to creation choice"
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
            } else if model.guided_project_recovery_active() {
                "Fields are retained · Enter check the same operation again · Esc recovery details"
            } else if model.guided_project_creation_active() {
                "Tab/Shift-Tab field · Enter create · Esc return to creation choice"
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
                    lines.push(Line::from(match &result.action {
                        UiProjectAction::CreateWorktree { name, .. } => {
                            format!("Creating the {name} worktree…")
                        }
                        UiProjectAction::CreateExisting { name, .. } => {
                            format!("Creating the {name} project…")
                        }
                        _ => "HQ is still finishing this change.".to_owned(),
                    }));
                    lines.push(Line::from(format!("Current stage: {stage}")));
                    lines.push(Line::from(
                        "This operation cannot be cancelled while Git work is in progress.",
                    ));
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
                    ..
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
            if !matches!(result.outcome, UiProjectOutcome::Running { .. }) {
                lines.push(Line::from("Esc close"));
            }
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
                UiMailboxAction::ArchiveConversation { .. } => (
                    "Archive this conversation?",
                    Some("HQ will stop active work first, then put away the whole conversation."),
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
        UiMailboxDraftTarget::ProjectSetup { .. } => "First project message",
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

fn render_rows(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    cache: &mut UiRenderCache,
) {
    if model.section() == UiSection::Config {
        render_config(frame, model, theme, area);
    } else if model.section() == UiSection::Projects {
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
            let conversation_owns_space = matches!(
                model.focus(),
                UiFocus::Conversation | UiFocus::Draft | UiFocus::Approval
            );
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

#[allow(clippy::too_many_lines)]
fn render_config(frame: &mut Frame<'_>, model: &UiModel, theme: &UiTheme, area: Rect) {
    let Some(configuration) = model.configuration() else {
        frame.render_widget(
            Paragraph::new("Loading configuration…")
                .style(theme.style(UiThemeRole::TextMuted))
                .block(Block::bordered().title(" Config ")),
            area,
        );
        return;
    };
    let value = |field| match field {
        UiConfigField::Theme => configuration
            .theme
            .clone()
            .unwrap_or_else(|| "automatic".to_owned()),
        UiConfigField::DefaultProvider => configuration
            .default_provider
            .clone()
            .unwrap_or_else(|| "default".to_owned()),
        UiConfigField::CodexModel => configuration
            .codex_model
            .clone()
            .unwrap_or_else(|| "default".to_owned()),
        UiConfigField::CodexYolo => {
            if configuration.codex_yolo {
                "enabled".to_owned()
            } else {
                "disabled".to_owned()
            }
        }
    };
    let label = |field| match field {
        UiConfigField::Theme => "Theme",
        UiConfigField::DefaultProvider => "Default provider",
        UiConfigField::CodexModel => "Codex model",
        UiConfigField::CodexYolo => "Codex yolo",
    };
    let mut lines = UiConfigField::ALL
        .into_iter()
        .map(|field| {
            let selected = field == model.config_field();
            let displayed = if selected {
                model
                    .config_edit()
                    .map_or_else(|| value(field), str::to_owned)
            } else {
                value(field)
            };
            Line::styled(
                format!(
                    "{}{}: {displayed}",
                    if selected { "› " } else { "  " },
                    label(field)
                ),
                if selected {
                    selected_style(theme, model.focus() == UiFocus::Content)
                } else {
                    theme.style(UiThemeRole::Text)
                },
            )
        })
        .collect::<Vec<_>>();
    lines.push(Line::default());
    lines.push(Line::styled(
        "Supported themes",
        theme.style(UiThemeRole::Heading),
    ));
    for choice in &configuration.themes {
        let active = choice.selector == configuration.theme;
        let selector = choice.selector.as_deref().unwrap_or("automatic");
        let suffix = choice.error.as_ref().map_or_else(
            || format!(" · {}", choice.source),
            |error| format!(" · {error}"),
        );
        lines.push(Line::styled(
            format!(
                "{} {selector} — {}{suffix}",
                if active { "●" } else { "○" },
                choice.name
            ),
            if choice.error.is_some() {
                theme.style(UiThemeRole::Error)
            } else if active {
                theme.style(UiThemeRole::Accent)
            } else {
                theme.style(UiThemeRole::TextMuted)
            },
        ));
    }
    let title = if model.configuration_pending() {
        " Config · Saving… "
    } else {
        " Config "
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(title).border_style(theme.style(
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
        format!("Projects / {} · ← Projects", summary.name)
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
            UiProjectWorkspaceLevel::Manage => "← Project summary",
            UiProjectWorkspaceLevel::Folders => "Tab choose folder · ← Manage project",
            UiProjectWorkspaceLevel::List => "",
            UiProjectWorkspaceLevel::Summary => "Enter choose · ← Projects",
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
    if let Some(approval) = model.current_command_approval() {
        let approval_height = (area.height / 3).max(7).min(area.height.saturating_sub(2));
        let [conversation, approval_area] =
            Layout::vertical([Constraint::Min(2), Constraint::Length(approval_height)]).areas(area);
        if model.technical_visible() {
            render_technical_inspector(frame, model, theme, conversation, outer_border, cache);
        } else {
            render_conversation(frame, model, theme, conversation, outer_border, cache);
        }
        render_command_approval(
            frame,
            model,
            theme,
            approval,
            approval_area,
            Borders::TOP | outer_border,
        );
    } else if model.mailbox_draft().is_some() && !model.draft_suspended_by_command_approval() {
        let draft_height = (area.height / 3).max(6).min(area.height.saturating_sub(2));
        let [conversation, draft] =
            Layout::vertical([Constraint::Min(2), Constraint::Length(draft_height)]).areas(area);
        if model.technical_visible() {
            render_technical_inspector(frame, model, theme, conversation, outer_border, cache);
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
        render_technical_inspector(
            frame,
            model,
            theme,
            inspector,
            Borders::TOP | outer_border,
            cache,
        );
    } else if model.technical_visible() {
        render_technical_inspector(frame, model, theme, area, outer_border, cache);
    } else {
        render_conversation(frame, model, theme, area, outer_border, cache);
    }
}

fn render_command_approval(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    approval: &crate::UiCommandApproval,
    area: Rect,
    borders: Borders,
) {
    let interaction = &approval.interaction;
    let title = interaction.project_name.as_ref().map_or_else(
        || format!(" Command for {} ", interaction.agent_name),
        |project| format!(" Command for {} · {project} ", interaction.agent_name),
    );
    let block = Block::new()
        .borders(borders)
        .title(Span::styled(title, theme.style(UiThemeRole::Attention)))
        .border_style(theme.style(if model.focus() == UiFocus::Approval {
            UiThemeRole::BorderFocused
        } else {
            UiThemeRole::BorderUnfocused
        }));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![Line::from(interaction.prompt.clone())];
    for (index, choice) in interaction.choices.iter().enumerate() {
        lines.push(Line::styled(
            format!(
                "{} {}",
                if index == approval.selected {
                    "›"
                } else {
                    " "
                },
                choice.label
            ),
            if index == approval.selected {
                selected_style(theme, model.focus() == UiFocus::Approval)
            } else {
                theme.style(UiThemeRole::Text)
            },
        ));
    }
    if let Some(failure) = &approval.failure {
        lines.push(Line::styled(
            format!("Could not send the response · {}", failure.action),
            theme.style(UiThemeRole::Error),
        ));
    }
    if model.technical_visible() {
        lines.push(Line::styled(
            format!(
                "provider={} · session={} · operation={} · request={}",
                interaction.provider,
                interaction.session,
                short_identity(interaction.operation_id),
                short_identity(interaction.request_id),
            ),
            theme.style(UiThemeRole::TextMuted),
        ));
    }
    lines.push(Line::styled(
        if approval.submitting.is_some() {
            "Sending your response…"
        } else if model.focus() == UiFocus::Approval {
            "↑/↓ or j/k choose · Enter confirm · Esc leave"
        } else {
            "Tab focus command choices"
        },
        theme.style(UiThemeRole::Footer),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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
            let content = draft_source_text(
                theme,
                &draft.content,
                cursor,
                model.focus() == UiFocus::Draft,
            );
            frame.render_widget(
                Paragraph::new(content).wrap(Wrap { trim: false }),
                text_area,
            );
            let hint_text = model
                .message_field_error()
                .unwrap_or("Enter send · Ctrl-J/Shift-Enter newline · Esc close");
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

#[cfg(test)]
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

fn draft_source_text(theme: &UiTheme, source: &str, cursor: usize, focused: bool) -> Text<'static> {
    let input_style = theme.style(UiThemeRole::Input);
    let cursor_style = input_style.patch(theme.style(UiThemeRole::Cursor));
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    for (offset, character) in source.char_indices() {
        if character == '\n' {
            if focused && offset == cursor {
                spans.push(Span::styled(" ", cursor_style));
            }
            lines.push(Line::from(std::mem::take(&mut spans)));
            continue;
        }
        let rendered = match character {
            '\t' => "    ".to_owned(),
            character if character.is_control() => " ".to_owned(),
            character => character.to_string(),
        };
        spans.push(Span::styled(
            rendered,
            if focused && offset == cursor {
                cursor_style
            } else {
                input_style
            },
        ));
    }
    if focused && cursor == source.len() {
        spans.push(Span::styled(" ", cursor_style));
    }
    lines.push(Line::from(spans));
    Text::from(lines)
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
    let unresolved = model.unresolved_command_approval_count();
    if unresolved > 0 {
        lines.push(Line::styled(
            format!(" {unresolved} command request(s) need conversation recovery."),
            theme.style(UiThemeRole::Attention),
        ));
        for interaction in model.unresolved_command_approvals() {
            let reason = match interaction.target {
                UiInteractionTarget::Unresolved {
                    reason: UiInteractionTargetIssue::Missing,
                } => "the conversation is not available",
                UiInteractionTarget::Unresolved {
                    reason: UiInteractionTargetIssue::Ambiguous,
                } => "more than one conversation matches",
                UiInteractionTarget::Unresolved {
                    reason: UiInteractionTargetIssue::OperationMismatch,
                } => "the running command does not match",
                UiInteractionTarget::Modal | UiInteractionTarget::Conversation { .. } => continue,
            };
            lines.push(Line::from(format!(
                " {}: {reason}.",
                interaction.agent_name
            )));
            if model.technical_visible() {
                lines.push(Line::styled(
                    format!(
                        " Technical: provider={} · session={} · operation={} · request={}",
                        interaction.provider,
                        interaction.session,
                        short_identity(interaction.operation_id),
                        short_identity(interaction.request_id)
                    ),
                    theme.style(UiThemeRole::TextTechnical),
                ));
            }
        }
        lines.push(Line::from(" Press F5 to refresh and place it safely."));
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
            Line::styled(" No agents are available yet.", heading),
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
        UiSection::Config => vec![Line::styled(" Loading configuration…", heading)],
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
    let block = Block::new().borders(divider).border_style(
        if matches!(model.focus(), UiFocus::Conversation | UiFocus::Approval) {
            theme.style(UiThemeRole::BorderFocused)
        } else {
            theme.style(UiThemeRole::BorderUnfocused)
        },
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(conversation) = conversation {
        let mut header = vec![Line::styled(
            conversation.title.as_str(),
            pane_title_style(
                theme,
                matches!(model.focus(), UiFocus::Conversation | UiFocus::Approval),
            ),
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
    } else if let Some(setup) = model.conversation_setup() {
        render_project_setup(
            frame,
            setup,
            theme,
            inner,
            model.focus() == UiFocus::Conversation,
        );
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

fn render_project_setup(
    frame: &mut Frame<'_>,
    setup: &crate::UiProjectConversationSetup,
    theme: &UiTheme,
    area: Rect,
    focused: bool,
) {
    let mut lines = vec![
        Line::styled(
            format!(
                "Conversation with {} about {} has not started",
                setup.agent_name, setup.project_name
            ),
            theme.style(UiThemeRole::Heading),
        ),
        Line::default(),
        Line::from(format!(
            "{} will be assigned when you send the first message.",
            setup.agent_name
        )),
        Line::from(format!("Agent service: {}", setup.provider_name)),
    ];
    if focused {
        lines.extend([
            Line::default(),
            Line::styled(
                "Press r or Enter to write the first message.",
                theme.style(UiThemeRole::Accent),
            ),
            Line::from("Press c to choose a different available agent."),
        ]);
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_technical_inspector(
    frame: &mut Frame<'_>,
    model: &UiModel,
    theme: &UiTheme,
    area: Rect,
    borders: Borders,
    cache: &mut UiRenderCache,
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
            lines.extend(completed_item_detail_styled(
                &entry.id,
                completed,
                inner.width,
                theme,
                cache,
            ));
        }
    }
    for section in &entry.technical {
        append_technical_section(&mut lines, section, theme);
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        "j/k scroll · t/← close details · ? help",
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
    let origin = if model.conversation_follows_tail() {
        conversation_viewport_tail_origin(&heights, area.height)
    } else {
        model
            .conversation_viewport_position()
            .and_then(|position| {
                visible_entries
                    .iter()
                    .position(|entry| entry.id == position.entry_id)
                    .map(|index| (index, position.row))
            })
            .unwrap_or_else(|| {
                (
                    model
                        .conversation_anchor()
                        .and_then(|anchor| {
                            visible_entries.iter().position(|entry| entry.id == anchor)
                        })
                        .unwrap_or(0),
                    0,
                )
            })
    };
    let visible = conversation_viewport_slices(&heights, origin.0, origin.1, area.height);
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    for slice in visible {
        if y >= bottom {
            break;
        }
        let entry = &visible_entries[slice.index];
        let layout = &layouts[slice.index];
        let height = slice.height.min(bottom - y);
        render_conversation_entry(
            frame,
            model,
            entry,
            layout,
            theme,
            slice.first_row,
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
    rows: Vec<Line<'static>>,
    height: u16,
}

fn conversation_entry_layout(
    entry: &UiConversationEntry,
    width: u16,
    theme: &UiTheme,
    cache: &mut UiRenderCache,
) -> ConversationEntryLayout {
    match &entry.presentation {
        UiConversationEntryPresentation::Message { author, body } => {
            let message = cache.messages.render(&entry.id, body, width, theme);
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
            let mut rows = Vec::with_capacity(usize::from(message.body_height()).saturating_add(2));
            rows.push(Line::styled(
                format!("{author}{delivery}{exceptional}"),
                theme.style(author_role),
            ));
            if message.text().lines.is_empty() {
                rows.push(Line::default());
            } else {
                rows.extend(message.text().lines.iter().cloned());
            }
            rows.push(Line::default());
            ConversationEntryLayout {
                height: u16::try_from(rows.len()).unwrap_or(u16::MAX),
                rows,
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
            let mut rows = activity_preview_lines(entry, width, symbol, role, theme, cache);
            rows.push(Line::default());
            ConversationEntryLayout {
                height: u16::try_from(rows.len()).unwrap_or(u16::MAX),
                rows,
            }
        }
    }
}

fn activity_preview_lines(
    entry: &UiConversationEntry,
    width: u16,
    symbol: &str,
    role: UiThemeRole,
    theme: &UiTheme,
    cache: &mut UiRenderCache,
) -> Vec<Line<'static>> {
    const PREVIEW_LINES: usize = 3;
    let completed_command_succeeded = matches!(
        &entry.presentation,
        UiConversationEntryPresentation::Activity {
            status: UiActivityStatus::Succeeded,
            completed: Some(UiCompletedItemPresentation::Command { .. }),
            ..
        }
    );
    let mut rows = activity_preview(entry, width)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            Line::styled(
                if index == 0 {
                    format!("{symbol} {line}")
                } else {
                    line
                },
                theme.style(if completed_command_succeeded && index > 0 {
                    UiThemeRole::Text
                } else {
                    role
                }),
            )
        })
        .collect::<Vec<_>>();
    let UiConversationEntryPresentation::Activity {
        completed: Some(UiCompletedItemPresentation::Command { command, .. }),
        ..
    } = &entry.presentation
    else {
        return rows;
    };
    let highlighted = cache.commands.highlight(&entry.id, command);
    for (index, segments) in highlighted.iter().take(PREVIEW_LINES).enumerate() {
        let prefix = if index == 0 { "$ " } else { "  " };
        if let Some(row) = rows.get_mut(index + 1) {
            *row = styled_shell_line(
                prefix,
                segments,
                usize::from(width),
                if completed_command_succeeded {
                    UiThemeRole::Text
                } else {
                    role
                },
                theme,
            );
        }
    }
    rows
}

fn styled_shell_line(
    prefix: &str,
    segments: &[ShellSegment],
    width: usize,
    base_role: UiThemeRole,
    theme: &UiTheme,
) -> Line<'static> {
    let mut spans = vec![Span::styled(prefix.to_owned(), theme.style(base_role))];
    let mut remaining = width.saturating_sub(UnicodeWidthStr::width(prefix));
    let total_width = segments
        .iter()
        .map(|segment| UnicodeWidthStr::width(segment.text.as_str()))
        .sum::<usize>();
    let clipped = total_width > remaining;
    if clipped {
        remaining = remaining.saturating_sub(1);
    }
    for segment in segments {
        if remaining == 0 {
            break;
        }
        let text = display_prefix(&segment.text, remaining);
        remaining = remaining.saturating_sub(UnicodeWidthStr::width(text));
        if !text.is_empty() {
            spans.push(Span::styled(
                text.to_owned(),
                shell_token_style(segment.kind, base_role, theme),
            ));
        }
    }
    if clipped && width > UnicodeWidthStr::width(prefix) {
        spans.push(Span::styled("…", theme.style(base_role)));
    }
    Line::from(spans)
}

fn shell_token_style(kind: ShellTokenKind, base_role: UiThemeRole, theme: &UiTheme) -> Style {
    let syntax_role = match kind {
        ShellTokenKind::Plain => return theme.style(base_role),
        ShellTokenKind::Comment => UiThemeRole::TextMuted,
        ShellTokenKind::Constant | ShellTokenKind::Number => UiThemeRole::Attention,
        ShellTokenKind::Function => UiThemeRole::Accent,
        ShellTokenKind::Keyword => UiThemeRole::Heading,
        ShellTokenKind::Operator | ShellTokenKind::Punctuation => UiThemeRole::Warning,
        ShellTokenKind::String => UiThemeRole::Success,
        ShellTokenKind::Variable => UiThemeRole::ConversationProjectContext,
    };
    theme.style(base_role).patch(theme.style(syntax_role))
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

fn completed_item_detail_styled(
    cache_key: &str,
    completed: &UiCompletedItemPresentation,
    width: u16,
    theme: &UiTheme,
    cache: &mut UiRenderCache,
) -> Vec<Line<'static>> {
    let mut lines = completed_item_detail_lines(completed)
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    let UiCompletedItemPresentation::Command { command, .. } = completed else {
        return lines;
    };
    let highlighted = cache.commands.highlight(cache_key, command);
    for (index, segments) in highlighted.iter().enumerate() {
        let prefix = if index == 0 { "$ " } else { "  " };
        if let Some(line) = lines.get_mut(index + 1) {
            *line = styled_shell_line(
                prefix,
                segments,
                usize::from(width),
                UiThemeRole::Text,
                theme,
            );
        }
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConversationViewportSlice {
    index: usize,
    first_row: u16,
    height: u16,
}

fn conversation_viewport_slices(
    heights: &[u16],
    top_index: usize,
    top_row: u16,
    available_height: u16,
) -> Vec<ConversationViewportSlice> {
    if heights.is_empty() || available_height == 0 {
        return Vec::new();
    }
    let top_index = top_index.min(heights.len() - 1);
    let mut first_row = top_row.min(heights[top_index].saturating_sub(1));
    let mut remaining = available_height;
    let mut slices = Vec::new();
    for (index, entry_height) in heights.iter().copied().enumerate().skip(top_index) {
        if remaining == 0 {
            break;
        }
        let height = entry_height.saturating_sub(first_row).min(remaining);
        if height > 0 {
            slices.push(ConversationViewportSlice {
                index,
                first_row,
                height,
            });
            remaining = remaining.saturating_sub(height);
        }
        first_row = 0;
    }
    slices
}

fn conversation_viewport_tail_origin(heights: &[u16], available_height: u16) -> (usize, u16) {
    let maximum_top = heights
        .iter()
        .map(|height| u64::from(*height))
        .sum::<u64>()
        .saturating_sub(u64::from(available_height));
    let mut start = 0_u64;
    for (index, height) in heights.iter().copied().enumerate() {
        let end = start.saturating_add(u64::from(height));
        if maximum_top < end {
            return (
                index,
                u16::try_from(maximum_top.saturating_sub(start)).unwrap_or(u16::MAX),
            );
        }
        start = end;
    }
    (0, 0)
}

fn render_conversation_entry(
    frame: &mut Frame<'_>,
    model: &UiModel,
    entry: &UiConversationEntry,
    layout: &ConversationEntryLayout,
    theme: &UiTheme,
    first_row: u16,
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
    let lines = layout
        .rows
        .iter()
        .skip(usize::from(first_row))
        .take(usize::from(area.height))
        .cloned()
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
    let continues_above = first_row > 0;
    let continues_below = first_row.saturating_add(area.height) < layout.height;
    if continues_above || continues_below {
        let cue = match (continues_above, continues_below) {
            (true, true) if area.height == 1 => "↕",
            (true, _) => "↑",
            (_, true) => "↓",
            (false, false) => "",
        };
        let cue_y = if continues_above {
            area.y
        } else {
            area.bottom() - 1
        };
        frame.render_widget(
            Paragraph::new(cue).style(theme.style(UiThemeRole::TextMuted)),
            Rect {
                x: area.right() - 1,
                y: cue_y,
                width: 1,
                height: 1,
            },
        );
        if continues_above && continues_below && area.height > 1 {
            frame.render_widget(
                Paragraph::new("↓").style(theme.style(UiThemeRole::TextMuted)),
                Rect {
                    x: area.right() - 1,
                    y: area.bottom() - 1,
                    width: 1,
                    height: 1,
                },
            );
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
    if matches!(
        row.kind,
        UiRowKind::Conversation | UiRowKind::ConversationSetup
    ) {
        let title_style = if selected {
            selected_style(theme, model.focus() == UiFocus::Content)
        } else if row.state == UiRowState::Unread {
            theme.style(UiThemeRole::RowAttention)
        } else {
            theme.style(UiThemeRole::Text)
        };
        let detail_style = if selected {
            title_style
        } else {
            theme.style(UiThemeRole::TextMuted)
        };
        let title = if row.state == UiRowState::Unread {
            format!("● {}", row.title)
        } else {
            row.title.clone()
        };
        return [
            Line::styled(padded_display_text(&title, width), title_style),
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
    } else if model.focus() == UiFocus::Approval {
        " j/k choose · Enter confirm · Esc leave · Tab switch pane · ? help · q quit".to_owned()
    } else if model.focus() == UiFocus::Draft {
        " Enter send · Ctrl-J/Shift-Enter newline · Esc close · ? help · q quit".to_owned()
    } else if model.section() == UiSection::Config {
        if model.config_edit().is_some() {
            " Enter save · Esc cancel · F1 help".to_owned()
        } else {
            " j/k choose · Enter/→ edit or change · ? help · q quit".to_owned()
        }
    } else if model.section() == UiSection::Projects {
        match model.project_workspace_level() {
            UiProjectWorkspaceLevel::List if model.selected_row_data().is_some() => {
                " Enter collaborate · → summary · c create · / search · ? help · q quit".to_owned()
            }
            UiProjectWorkspaceLevel::List => " c create · / search · ? help · q quit".to_owned(),
            UiProjectWorkspaceLevel::Summary => {
                " j/k choose · Enter open · ← Projects · ? help · q quit".to_owned()
            }
            UiProjectWorkspaceLevel::Manage => {
                " j/k choose action · Enter open · ← summary · ? help · q quit".to_owned()
            }
            UiProjectWorkspaceLevel::Folders => {
                " j/k choose action · Tab folder · Enter run · ← manage · ? help · q quit"
                    .to_owned()
            }
        }
    } else if model.section() == UiSection::Agents {
        if model.selected_row_data().is_some() {
            " Enter inspect · n New… · c create · / search · ? help · q quit".to_owned()
        } else {
            " n New… · c create · / search · ? help · q quit".to_owned()
        }
    } else if model.conversation_setup().is_some() {
        " r/Enter write first message · c change agent · Tab switch pane · ? help · q quit"
            .to_owned()
    } else if model.focus() == UiFocus::Conversation {
        conversation_footer(model)
    } else if model.selected_row_data().is_none() {
        " n New… · ? help · q quit".to_owned()
    } else if model.viewport().width >= WIDE_WIDTH {
        if model.section() == UiSection::Inbox {
            " Enter open · n New… · d archive conversation · ? help · q quit".to_owned()
        } else {
            " Enter open · n New… · ? help · q quit".to_owned()
        }
    } else {
        " Enter open · ? help · q quit".to_owned()
    };
    let view_shortcuts_available = model.help_page().is_none()
        && model.new_modal().is_none()
        && model.mailbox_modal().is_none()
        && (model.mailbox_draft().is_none() || model.draft_suspended_by_command_approval())
        && model.agent_modal().is_none()
        && model.project_interaction().is_none()
        && model.interaction_modal().is_none()
        && model.config_edit().is_none();
    let content = if view_shortcuts_available {
        format!("{content} · 1–6 views")
    } else {
        content
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
        return " j/k scroll · t/← close details · ? help".to_owned();
    }
    let mut controls = vec![if model.viewport().width >= WIDE_WIDTH {
        "↑/↓ or j/k message"
    } else {
        "j/k"
    }];
    if matches!(
        model
            .selected_row_data()
            .and_then(|row| row.conversation_target.as_ref()),
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
    if model.section() == UiSection::Inbox {
        controls.push("d archive conversation");
    }
    controls.push(if model.viewport().width >= WIDE_WIDTH {
        "t/Enter details"
    } else {
        "t/Enter info"
    });
    controls.push("← Inbox");
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
        UiSection::Config => "Config",
    }
}

const fn row_state_label(section: UiSection, state: UiRowState) -> &'static str {
    match (section, state) {
        (UiSection::Sent, UiRowState::Waiting) => "sent",
        (_, UiRowState::Open) => "open",
        (_, UiRowState::Unread) => "unread",
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
        (UiSection::Config, _) => "settings",
    }
}

fn row_state_style(theme: &UiTheme, state: UiRowState) -> Style {
    theme.style(match state {
        UiRowState::Open => UiThemeRole::RowOpen,
        UiRowState::Unread | UiRowState::Attention => UiThemeRole::RowAttention,
        UiRowState::Waiting => UiThemeRole::RowWaiting,
        UiRowState::Archived => UiThemeRole::RowArchived,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        activity_preview, completed_item_detail_lines, completed_item_detail_styled,
        conversation_entry_layout, conversation_viewport_slices, conversation_viewport_tail_origin,
        display_prefix, display_suffix, draft_context_label, inbox_list_width, inert_draft_source,
        interaction_supersedes_live_tail, render_conversation_entries, render_conversation_entry,
        text_field_line,
    };
    use crate::{
        UiActivityStatus, UiCompletedItemPresentation, UiConversationActivityKind,
        UiConversationAuthor, UiConversationEntry, UiConversationEntryPresentation,
        UiConversationViewportObservation, UiInteraction, UiInteractionKind, UiMessageState,
        UiModel, UiSize, UiTechnicalSection, UiTheme, UiThemeRole,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use unicode_width::UnicodeWidthStr;

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
            target: crate::UiInteractionTarget::Conversation {
                row_id: "conversation".to_owned(),
            },
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
        let theme = UiTheme::terminal();
        let layout = conversation_entry_layout(&entry, 80, &theme, &mut cache);
        assert_eq!(
            layout.height,
            u16::try_from(preview.len())
                .unwrap_or(u16::MAX)
                .saturating_add(1)
        );
        let command = layout
            .rows
            .iter()
            .find(|line| line.to_string().contains("printf one"));
        assert!(command.is_some(), "highlighted command line missing");
        if let Some(command) = command {
            assert!(command.spans.iter().any(|span| {
                span.style
                    == theme
                        .style(UiThemeRole::ConversationActivityError)
                        .patch(theme.style(UiThemeRole::Accent))
            }));
        }
    }

    #[test]
    fn completed_command_preview_colors_only_the_status_as_success() {
        let entry = UiConversationEntry {
            id: "completed-command".to_owned(),
            presentation: UiConversationEntryPresentation::Activity {
                kind: UiConversationActivityKind::CompletedItem,
                status: UiActivityStatus::Succeeded,
                summary: "Command completed".to_owned(),
                detail: "complete detail".to_owned(),
                truncated: false,
                completed: Some(UiCompletedItemPresentation::Command {
                    command: "printf '%s\\n' done".to_owned(),
                    output: Some("done".to_owned()),
                    exit_code: Some(0),
                    command_truncated: false,
                    output_truncated: false,
                }),
            },
            message_state: None,
            delivery: None,
            message_target: None,
            technical: Vec::new(),
        };
        let theme = UiTheme::terminal();
        let mut cache = super::UiRenderCache::new();
        let layout = conversation_entry_layout(&entry, 80, &theme, &mut cache);

        assert_eq!(
            layout.rows[0].style,
            theme.style(UiThemeRole::ConversationActivitySuccess)
        );
        assert_eq!(layout.rows[2].to_string(), "│ done");
        assert_eq!(layout.rows[2].style, theme.style(UiThemeRole::Text));
        assert!(layout.rows[1].spans.iter().any(|span| {
            span.style
                == theme
                    .style(UiThemeRole::Text)
                    .patch(theme.style(UiThemeRole::Success))
        }));
        assert_eq!(
            layout.rows[1].spans[0].style,
            theme.style(UiThemeRole::Text)
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
        let theme = UiTheme::terminal();
        let mut cache = super::UiRenderCache::new();
        let styled =
            completed_item_detail_styled("command-detail", &completed, 80, &theme, &mut cache);
        assert!(styled[1].spans.len() > 1);
        assert_eq!(styled[4].to_string(), "│ one");
    }

    #[test]
    fn continuous_viewport_slices_every_intersecting_entry() {
        assert_eq!(
            conversation_viewport_slices(&[3, 12, 3], 1, 4, 5),
            vec![super::ConversationViewportSlice {
                index: 1,
                first_row: 4,
                height: 5,
            }]
        );
        assert_eq!(
            conversation_viewport_slices(&[3, 12, 3], 0, 2, 5),
            vec![
                super::ConversationViewportSlice {
                    index: 0,
                    first_row: 2,
                    height: 1,
                },
                super::ConversationViewportSlice {
                    index: 1,
                    first_row: 0,
                    height: 4,
                },
            ]
        );
        assert!(conversation_viewport_slices(&[3], 0, 0, 0).is_empty());
        assert_eq!(conversation_viewport_slices(&[8], 0, 3, 1)[0].height, 1);
        assert_eq!(conversation_viewport_slices(&[8], 0, 3, 2)[0].height, 2);
        assert_eq!(conversation_viewport_tail_origin(&[3, 20], 5), (1, 15));
        assert_eq!(conversation_viewport_tail_origin(&[20, 3], 5), (0, 18));
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

    #[test]
    fn entry_painting_slices_cached_markdown_rows_with_continuation_cues() {
        let entry = message("# One\n\n# Two\n\n# Three");
        let model = UiModel::new(UiSize {
            width: 40,
            height: 8,
        });
        let theme = UiTheme::terminal();
        let mut cache = super::UiRenderCache::new();
        let layout = conversation_entry_layout(&entry, 16, &theme, &mut cache);
        let middle = layout
            .rows
            .iter()
            .position(|line| line.to_string().contains("Two"))
            .unwrap_or(usize::MAX);
        assert!(middle > 0 && middle + 1 < layout.rows.len());
        let mut terminal = Terminal::new(TestBackend::new(16, 1)).expect("test terminal");

        terminal
            .draw(|frame| {
                render_conversation_entry(
                    frame,
                    &model,
                    &entry,
                    &layout,
                    &theme,
                    u16::try_from(middle).unwrap_or(u16::MAX),
                    Rect::new(0, 0, 16, 1),
                );
            })
            .expect("render middle message slice");

        let buffer = terminal.backend().buffer();
        let row = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(row.contains("Two"), "{row}");
        assert!(row.ends_with('↕'), "{row}");
        assert!(
            buffer
                .content()
                .iter()
                .find(|cell| cell.symbol() == "T")
                .is_some_and(|cell| cell.modifier.contains(ratatui::style::Modifier::BOLD))
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
