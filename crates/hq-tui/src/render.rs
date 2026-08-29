//! Borrowed responsive Ratatui renderer.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{UiConnectionState, UiFocus, UiModel, UiRow, UiRowState, UiSection};

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
        || " tab focus · ↑/↓ select · ←/→ section · q quit".to_owned(),
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
