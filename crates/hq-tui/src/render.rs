//! Borrowed responsive Ratatui renderer.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    UiActivityStatus, UiConnectionState, UiConversationEntry, UiConversationEntryKind, UiFocus,
    UiMailboxAction, UiMailboxDraftTarget, UiMailboxModal, UiMessageState, UiModel, UiRow,
    UiRowState, UiSection, UiTechnicalSection,
};

const MINIMUM_WIDTH: u16 = 40;
const MINIMUM_HEIGHT: u16 = 10;
const WIDE_WIDTH: u16 = 96;
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
    let count = model.snapshot().map_or(0, |snapshot| snapshot.rows.len());
    let mut lines = vec![
        Line::styled(
            format!(" {} · {count} items", section_label(model.section())),
            Style::new().fg(Color::Cyan).bold(),
        ),
        Line::default(),
    ];
    match model.snapshot() {
        Some(snapshot) if snapshot.rows.is_empty() => {
            lines.push(Line::styled(" No items", Style::new().fg(Color::DarkGray)));
        }
        Some(snapshot) => {
            for row in &snapshot.rows {
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
            if model.viewport().width >= WIDE_WIDTH {
                " tab focus · ↑/↓ select · r reply · d direct · n note · a/u state · q quit"
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
