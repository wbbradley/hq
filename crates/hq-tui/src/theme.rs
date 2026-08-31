//! Complete semantic styles for borrowed TUI rendering.

use ratatui::style::{Color, Style};

const ROLE_COUNT: usize = 42;

/// Every independently configurable visual role in the HQ terminal interface.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(usize)]
pub enum UiThemeRole {
    /// Full terminal frame, including otherwise blank cells.
    Screen,
    /// Ordinary interface text.
    Text,
    /// De-emphasized explanatory text.
    TextMuted,
    /// Progressively disclosed technical evidence.
    TextTechnical,
    /// Section and dialog headings.
    Heading,
    /// Title of the pane that currently owns keyboard focus.
    PaneTitleFocused,
    /// Title of a selected pane that does not currently own keyboard focus.
    PaneTitleUnfocused,
    /// Primary interactive accent.
    Accent,
    /// Reserved local-human author label in a conversation.
    ConversationAuthorSelf,
    /// Named or fallback counterparty author label in a conversation.
    ConversationAuthorParticipant,
    /// Neutral or running compact conversation activity.
    ConversationActivity,
    /// Successful compact conversation activity.
    ConversationActivitySuccess,
    /// Interrupted or cautionary compact conversation activity.
    ConversationActivityWarning,
    /// Failed compact conversation activity.
    ConversationActivityError,
    /// Full-row transcript selection while Conversation owns focus.
    ConversationSelectionFocused,
    /// Full-row transcript selection retained behind details or a draft.
    ConversationSelectionUnfocused,
    /// Selected item while its control owns focus.
    SelectionFocused,
    /// Selected item while another control owns focus.
    SelectionUnfocused,
    /// Border around a focused control.
    BorderFocused,
    /// Border around an unfocused control.
    BorderUnfocused,
    /// Complete cleared surface behind a modal.
    ModalSurface,
    /// Modal border.
    ModalBorder,
    /// Modal title.
    ModalTitle,
    /// High-contrast badge in the application header.
    HeaderBadge,
    /// Editable text and choice input.
    Input,
    /// Complete one-line editable field surface when it does not own focus.
    InputField,
    /// Complete one-line editable field surface while it owns focus.
    InputFieldFocused,
    /// Text insertion cursor.
    Cursor,
    /// Ordinary footer and key guidance.
    Footer,
    /// Successful completion footer.
    FooterSuccess,
    /// Warning or recovery footer.
    FooterWarning,
    /// Ready connection status.
    ConnectionReady,
    /// Connecting or reconnecting status.
    ConnectionPending,
    /// Offline or incompatible connection status.
    ConnectionError,
    /// Open row state.
    RowOpen,
    /// Waiting row state.
    RowWaiting,
    /// Archived row state.
    RowArchived,
    /// Row requiring attention.
    RowAttention,
    /// Successful inline feedback.
    Success,
    /// Warning inline feedback.
    Warning,
    /// Error inline feedback.
    Error,
    /// Strong attention inline feedback.
    Attention,
}

impl UiThemeRole {
    /// Complete role inventory in stable configuration order.
    pub const ALL: [Self; ROLE_COUNT] = [
        Self::Screen,
        Self::Text,
        Self::TextMuted,
        Self::TextTechnical,
        Self::Heading,
        Self::PaneTitleFocused,
        Self::PaneTitleUnfocused,
        Self::Accent,
        Self::ConversationAuthorSelf,
        Self::ConversationAuthorParticipant,
        Self::ConversationActivity,
        Self::ConversationActivitySuccess,
        Self::ConversationActivityWarning,
        Self::ConversationActivityError,
        Self::ConversationSelectionFocused,
        Self::ConversationSelectionUnfocused,
        Self::SelectionFocused,
        Self::SelectionUnfocused,
        Self::BorderFocused,
        Self::BorderUnfocused,
        Self::ModalSurface,
        Self::ModalBorder,
        Self::ModalTitle,
        Self::HeaderBadge,
        Self::Input,
        Self::InputField,
        Self::InputFieldFocused,
        Self::Cursor,
        Self::Footer,
        Self::FooterSuccess,
        Self::FooterWarning,
        Self::ConnectionReady,
        Self::ConnectionPending,
        Self::ConnectionError,
        Self::RowOpen,
        Self::RowWaiting,
        Self::RowArchived,
        Self::RowAttention,
        Self::Success,
        Self::Warning,
        Self::Error,
        Self::Attention,
    ];

    /// Stable key used by native theme files and documentation.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Screen => "ui.screen",
            Self::Text => "ui.text",
            Self::TextMuted => "ui.text.muted",
            Self::TextTechnical => "ui.text.technical",
            Self::Heading => "ui.heading",
            Self::PaneTitleFocused => "ui.pane.title.focused",
            Self::PaneTitleUnfocused => "ui.pane.title.unfocused",
            Self::Accent => "ui.accent",
            Self::ConversationAuthorSelf => "conversation.author.self",
            Self::ConversationAuthorParticipant => "conversation.author.participant",
            Self::ConversationActivity => "conversation.activity",
            Self::ConversationActivitySuccess => "conversation.activity.success",
            Self::ConversationActivityWarning => "conversation.activity.warning",
            Self::ConversationActivityError => "conversation.activity.error",
            Self::ConversationSelectionFocused => "conversation.selection.focused",
            Self::ConversationSelectionUnfocused => "conversation.selection.unfocused",
            Self::SelectionFocused => "ui.selection.focused",
            Self::SelectionUnfocused => "ui.selection.unfocused",
            Self::BorderFocused => "ui.border.focused",
            Self::BorderUnfocused => "ui.border.unfocused",
            Self::ModalSurface => "ui.modal.surface",
            Self::ModalBorder => "ui.modal.border",
            Self::ModalTitle => "ui.modal.title",
            Self::HeaderBadge => "ui.header.badge",
            Self::Input => "ui.input",
            Self::InputField => "ui.input.field",
            Self::InputFieldFocused => "ui.input.field.focused",
            Self::Cursor => "ui.cursor",
            Self::Footer => "ui.footer",
            Self::FooterSuccess => "ui.footer.success",
            Self::FooterWarning => "ui.footer.warning",
            Self::ConnectionReady => "status.connection.ready",
            Self::ConnectionPending => "status.connection.pending",
            Self::ConnectionError => "status.connection.error",
            Self::RowOpen => "status.row.open",
            Self::RowWaiting => "status.row.waiting",
            Self::RowArchived => "status.row.archived",
            Self::RowAttention => "status.row.attention",
            Self::Success => "status.success",
            Self::Warning => "status.warning",
            Self::Error => "status.error",
            Self::Attention => "status.attention",
        }
    }

    /// Resolves one exact native theme key.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.key() == key)
    }
}

/// A complete 16-color Tinted/Base16 palette supplied by an outer parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Base16Palette {
    colors: [Color; 16],
}

impl Base16Palette {
    /// Creates one complete palette in `base00` through `base0F` order.
    pub const fn new(colors: [Color; 16]) -> Self {
        Self { colors }
    }

    const fn color(self, index: usize) -> Color {
        self.colors[index]
    }
}

/// A complete, resolved semantic theme borrowed by the renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiTheme {
    name: String,
    author: Option<String>,
    styles: [Style; ROLE_COUNT],
}

impl UiTheme {
    /// Reproduces HQ's terminal-native compatibility appearance.
    #[allow(
        clippy::too_many_lines,
        reason = "complete closed semantic role catalog"
    )]
    pub fn terminal() -> Self {
        let mut styles = [Style::new(); ROLE_COUNT];
        set(
            &mut styles,
            UiThemeRole::Screen,
            Style::new().fg(Color::Reset).bg(Color::Reset),
        );
        set(
            &mut styles,
            UiThemeRole::TextMuted,
            Style::new().fg(Color::DarkGray),
        );
        set(
            &mut styles,
            UiThemeRole::TextTechnical,
            Style::new().fg(Color::DarkGray),
        );
        set(
            &mut styles,
            UiThemeRole::Heading,
            Style::new().fg(Color::Cyan).bold(),
        );
        set(
            &mut styles,
            UiThemeRole::PaneTitleFocused,
            Style::new().fg(Color::Cyan).bold(),
        );
        set(
            &mut styles,
            UiThemeRole::PaneTitleUnfocused,
            Style::new().fg(Color::DarkGray).bold(),
        );
        set(
            &mut styles,
            UiThemeRole::Accent,
            Style::new().fg(Color::Cyan),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationAuthorSelf,
            Style::new().fg(Color::Cyan).bold(),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationAuthorParticipant,
            Style::new().fg(Color::Magenta).bold(),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationActivity,
            Style::new().fg(Color::DarkGray),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationActivitySuccess,
            Style::new().fg(Color::Green),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationActivityWarning,
            Style::new().fg(Color::Yellow),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationActivityError,
            Style::new().fg(Color::Red),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationSelectionFocused,
            Style::new().bold().reversed(),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationSelectionUnfocused,
            Style::new().bold(),
        );
        set(
            &mut styles,
            UiThemeRole::SelectionFocused,
            Style::new().fg(Color::Cyan).bold().reversed(),
        );
        set(
            &mut styles,
            UiThemeRole::SelectionUnfocused,
            Style::new().fg(Color::DarkGray).bold(),
        );
        set(
            &mut styles,
            UiThemeRole::BorderFocused,
            Style::new().fg(Color::Cyan),
        );
        set(
            &mut styles,
            UiThemeRole::BorderUnfocused,
            Style::new().fg(Color::DarkGray),
        );
        set(
            &mut styles,
            UiThemeRole::ModalSurface,
            Style::new().fg(Color::Reset).bg(Color::Reset),
        );
        set(
            &mut styles,
            UiThemeRole::ModalBorder,
            Style::new().fg(Color::Cyan),
        );
        set(
            &mut styles,
            UiThemeRole::ModalTitle,
            Style::new().fg(Color::Cyan).bold(),
        );
        set(
            &mut styles,
            UiThemeRole::HeaderBadge,
            Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
        );
        set(&mut styles, UiThemeRole::Input, Style::new());
        set(
            &mut styles,
            UiThemeRole::InputField,
            Style::new().fg(Color::Reset).bg(Color::DarkGray),
        );
        set(
            &mut styles,
            UiThemeRole::InputFieldFocused,
            Style::new().fg(Color::Black).bg(Color::Cyan),
        );
        set(&mut styles, UiThemeRole::Cursor, Style::new().reversed());
        set(
            &mut styles,
            UiThemeRole::Footer,
            Style::new().fg(Color::DarkGray),
        );
        set(
            &mut styles,
            UiThemeRole::FooterSuccess,
            Style::new().fg(Color::Green),
        );
        set(
            &mut styles,
            UiThemeRole::FooterWarning,
            Style::new().fg(Color::Yellow),
        );
        set(
            &mut styles,
            UiThemeRole::ConnectionReady,
            Style::new().fg(Color::Green),
        );
        set(
            &mut styles,
            UiThemeRole::ConnectionPending,
            Style::new().fg(Color::Yellow),
        );
        set(
            &mut styles,
            UiThemeRole::ConnectionError,
            Style::new().fg(Color::Red),
        );
        set(
            &mut styles,
            UiThemeRole::RowOpen,
            Style::new().fg(Color::Green),
        );
        set(
            &mut styles,
            UiThemeRole::RowWaiting,
            Style::new().fg(Color::Yellow),
        );
        set(
            &mut styles,
            UiThemeRole::RowArchived,
            Style::new().fg(Color::DarkGray),
        );
        set(
            &mut styles,
            UiThemeRole::RowAttention,
            Style::new().fg(Color::Red),
        );
        set(
            &mut styles,
            UiThemeRole::Success,
            Style::new().fg(Color::Green),
        );
        set(
            &mut styles,
            UiThemeRole::Warning,
            Style::new().fg(Color::Yellow),
        );
        set(&mut styles, UiThemeRole::Error, Style::new().fg(Color::Red));
        set(
            &mut styles,
            UiThemeRole::Attention,
            Style::new().fg(Color::Yellow).bold(),
        );
        Self {
            name: "terminal".to_owned(),
            author: None,
            styles,
        }
    }

    /// Uses terminal defaults and non-color modifiers only.
    #[allow(
        clippy::too_many_lines,
        reason = "complete closed semantic role catalog"
    )]
    pub fn no_color() -> Self {
        let mut styles = [Style::new(); ROLE_COUNT];
        set(
            &mut styles,
            UiThemeRole::Screen,
            Style::new().fg(Color::Reset).bg(Color::Reset),
        );
        set(&mut styles, UiThemeRole::TextMuted, Style::new().dim());
        set(&mut styles, UiThemeRole::TextTechnical, Style::new().dim());
        set(&mut styles, UiThemeRole::Heading, Style::new().bold());
        set(
            &mut styles,
            UiThemeRole::PaneTitleFocused,
            Style::new().bold().underlined(),
        );
        set(
            &mut styles,
            UiThemeRole::PaneTitleUnfocused,
            Style::new().bold(),
        );
        set(&mut styles, UiThemeRole::Accent, Style::new().bold());
        set(
            &mut styles,
            UiThemeRole::ConversationAuthorSelf,
            Style::new().bold(),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationAuthorParticipant,
            Style::new().bold(),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationActivity,
            Style::new().dim(),
        );
        for role in [
            UiThemeRole::ConversationActivitySuccess,
            UiThemeRole::ConversationActivityWarning,
            UiThemeRole::ConversationActivityError,
        ] {
            set(&mut styles, role, Style::new().bold());
        }
        set(
            &mut styles,
            UiThemeRole::ConversationSelectionFocused,
            Style::new().bold().reversed(),
        );
        set(
            &mut styles,
            UiThemeRole::ConversationSelectionUnfocused,
            Style::new().bold(),
        );
        set(
            &mut styles,
            UiThemeRole::SelectionFocused,
            Style::new().bold().reversed(),
        );
        set(
            &mut styles,
            UiThemeRole::SelectionUnfocused,
            Style::new().bold(),
        );
        set(&mut styles, UiThemeRole::BorderFocused, Style::new().bold());
        set(
            &mut styles,
            UiThemeRole::ModalSurface,
            Style::new().fg(Color::Reset).bg(Color::Reset),
        );
        set(&mut styles, UiThemeRole::ModalTitle, Style::new().bold());
        set(
            &mut styles,
            UiThemeRole::HeaderBadge,
            Style::new().bold().reversed(),
        );
        set(
            &mut styles,
            UiThemeRole::InputField,
            Style::new().dim().underlined(),
        );
        set(
            &mut styles,
            UiThemeRole::InputFieldFocused,
            Style::new().bold().reversed(),
        );
        set(&mut styles, UiThemeRole::Cursor, Style::new().reversed());
        for role in [
            UiThemeRole::FooterSuccess,
            UiThemeRole::FooterWarning,
            UiThemeRole::ConnectionError,
            UiThemeRole::RowAttention,
            UiThemeRole::Success,
            UiThemeRole::Warning,
            UiThemeRole::Error,
            UiThemeRole::Attention,
        ] {
            set(&mut styles, role, Style::new().bold());
        }
        Self {
            name: "no-color".to_owned(),
            author: None,
            styles,
        }
    }

    /// Deterministically maps one Tinted/Base16 palette into every HQ role.
    pub fn from_base16(name: String, author: Option<String>, palette: Base16Palette) -> Self {
        let background = palette.color(0x00);
        let surface = palette.color(0x01);
        let selection = palette.color(0x02);
        let muted = palette.color(0x03);
        let text = palette.color(0x05);
        let error = palette.color(0x08);
        let warning = palette.color(0x0A);
        let success = palette.color(0x0B);
        let accent = palette.color(0x0D);
        let participant = palette.color(0x0E);
        let mut theme = Self::terminal();
        theme.name = name;
        theme.author = author;
        theme.styles = [Style::new(); ROLE_COUNT];
        theme = theme
            .with_style(UiThemeRole::Screen, Style::new().fg(text).bg(background))
            .with_style(UiThemeRole::Text, Style::new().fg(text))
            .with_style(UiThemeRole::TextMuted, Style::new().fg(muted))
            .with_style(UiThemeRole::TextTechnical, Style::new().fg(muted).dim())
            .with_style(UiThemeRole::Heading, Style::new().fg(accent).bold())
            .with_style(
                UiThemeRole::PaneTitleFocused,
                Style::new().fg(accent).bold(),
            )
            .with_style(
                UiThemeRole::PaneTitleUnfocused,
                Style::new().fg(muted).bold(),
            )
            .with_style(UiThemeRole::Accent, Style::new().fg(accent))
            .with_style(
                UiThemeRole::ConversationAuthorSelf,
                Style::new().fg(accent).bold(),
            )
            .with_style(
                UiThemeRole::ConversationAuthorParticipant,
                Style::new().fg(participant).bold(),
            )
            .with_style(UiThemeRole::ConversationActivity, Style::new().fg(muted))
            .with_style(
                UiThemeRole::ConversationActivitySuccess,
                Style::new().fg(success),
            )
            .with_style(
                UiThemeRole::ConversationActivityWarning,
                Style::new().fg(warning),
            )
            .with_style(
                UiThemeRole::ConversationActivityError,
                Style::new().fg(error),
            )
            .with_style(
                UiThemeRole::ConversationSelectionFocused,
                Style::new().fg(text).bg(selection).bold(),
            )
            .with_style(
                UiThemeRole::ConversationSelectionUnfocused,
                Style::new().fg(text).bg(surface),
            )
            .with_style(
                UiThemeRole::SelectionFocused,
                Style::new().fg(text).bg(selection).bold(),
            )
            .with_style(
                UiThemeRole::SelectionUnfocused,
                Style::new().fg(text).bg(surface),
            )
            .with_style(UiThemeRole::BorderFocused, Style::new().fg(accent))
            .with_style(UiThemeRole::BorderUnfocused, Style::new().fg(selection))
            .with_style(UiThemeRole::ModalSurface, Style::new().fg(text).bg(surface))
            .with_style(UiThemeRole::ModalBorder, Style::new().fg(accent))
            .with_style(UiThemeRole::ModalTitle, Style::new().fg(accent).bold())
            .with_style(
                UiThemeRole::HeaderBadge,
                Style::new().fg(background).bg(accent).bold(),
            )
            .with_style(UiThemeRole::Input, Style::new().fg(text).bg(surface))
            .with_style(UiThemeRole::InputField, Style::new().fg(text).bg(selection))
            .with_style(
                UiThemeRole::InputFieldFocused,
                Style::new().fg(background).bg(accent).bold(),
            )
            .with_style(UiThemeRole::Cursor, Style::new().fg(background).bg(text))
            .with_style(UiThemeRole::Footer, Style::new().fg(muted))
            .with_style(UiThemeRole::FooterSuccess, Style::new().fg(success))
            .with_style(UiThemeRole::FooterWarning, Style::new().fg(warning))
            .with_style(UiThemeRole::ConnectionReady, Style::new().fg(success))
            .with_style(UiThemeRole::ConnectionPending, Style::new().fg(warning))
            .with_style(UiThemeRole::ConnectionError, Style::new().fg(error))
            .with_style(UiThemeRole::RowOpen, Style::new().fg(success))
            .with_style(UiThemeRole::RowWaiting, Style::new().fg(warning))
            .with_style(UiThemeRole::RowArchived, Style::new().fg(muted))
            .with_style(UiThemeRole::RowAttention, Style::new().fg(error))
            .with_style(UiThemeRole::Success, Style::new().fg(success))
            .with_style(UiThemeRole::Warning, Style::new().fg(warning))
            .with_style(UiThemeRole::Error, Style::new().fg(error))
            .with_style(UiThemeRole::Attention, Style::new().fg(warning).bold());
        theme
    }

    /// Returns the display name carried by this resolved theme.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns optional source attribution.
    pub fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }

    /// Returns the complete style for one semantic role.
    pub const fn style(&self, role: UiThemeRole) -> Style {
        self.styles[role as usize]
    }

    /// Returns a copy with one semantic role replaced.
    #[must_use]
    pub fn with_style(mut self, role: UiThemeRole, style: Style) -> Self {
        self.styles[role as usize] = style;
        self
    }

    /// Returns a copy with bounded, already-validated display metadata.
    #[must_use]
    pub fn with_metadata(mut self, name: String, author: Option<String>) -> Self {
        self.name = name;
        self.author = author;
        self
    }
}

fn set(styles: &mut [Style; ROLE_COUNT], role: UiThemeRole, style: Style) {
    styles[role as usize] = style;
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier, Style};

    use super::{Base16Palette, UiTheme, UiThemeRole};

    #[test]
    fn role_keys_are_complete_unique_and_round_trip() {
        let mut keys = UiThemeRole::ALL.map(UiThemeRole::key);
        keys.sort_unstable();
        assert!(keys.windows(2).all(|pair| pair[0] != pair[1]));
        for role in UiThemeRole::ALL {
            assert_eq!(UiThemeRole::from_key(role.key()), Some(role));
        }
        assert_eq!(UiThemeRole::from_key("ui.unknown"), None);
    }

    #[test]
    fn no_color_retains_focus_without_non_reset_colors() {
        let theme = UiTheme::no_color();
        for role in UiThemeRole::ALL {
            let style = theme.style(role);
            assert!(style.fg.is_none_or(|color| color == Color::Reset));
            assert!(style.bg.is_none_or(|color| color == Color::Reset));
        }
        assert!(
            theme
                .style(UiThemeRole::SelectionFocused)
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn base16_mapping_fills_every_semantic_status() {
        let colors =
            std::array::from_fn(|index| Color::Indexed(u8::try_from(index).unwrap_or(u8::MAX)));
        let theme = UiTheme::from_base16(
            "example".to_owned(),
            Some("Theme Author".to_owned()),
            Base16Palette::new(colors),
        );
        assert_eq!(theme.name(), "example");
        assert_eq!(theme.author(), Some("Theme Author"));
        assert_eq!(theme.style(UiThemeRole::Screen).bg, Some(Color::Indexed(0)));
        assert_eq!(theme.style(UiThemeRole::Text).fg, Some(Color::Indexed(5)));
        assert_eq!(theme.style(UiThemeRole::Error).fg, Some(Color::Indexed(8)));
        assert_eq!(
            theme.style(UiThemeRole::Warning).fg,
            Some(Color::Indexed(10))
        );
        assert_eq!(
            theme.style(UiThemeRole::Success).fg,
            Some(Color::Indexed(11))
        );
        assert_eq!(
            theme.style(UiThemeRole::Accent).fg,
            Some(Color::Indexed(13))
        );
        assert_eq!(
            theme.style(UiThemeRole::ConversationAuthorSelf).fg,
            Some(Color::Indexed(13))
        );
        assert_eq!(
            theme.style(UiThemeRole::ConversationAuthorParticipant).fg,
            Some(Color::Indexed(14))
        );
        assert_eq!(
            theme.style(UiThemeRole::ConversationActivity).fg,
            Some(Color::Indexed(3))
        );
        assert_eq!(
            theme.style(UiThemeRole::ConversationSelectionFocused).bg,
            Some(Color::Indexed(2))
        );
    }

    #[test]
    fn no_color_conversation_roles_retain_text_and_focus_cues() {
        let theme = UiTheme::no_color();
        for role in [
            UiThemeRole::ConversationAuthorSelf,
            UiThemeRole::ConversationAuthorParticipant,
            UiThemeRole::ConversationActivitySuccess,
            UiThemeRole::ConversationActivityWarning,
            UiThemeRole::ConversationActivityError,
        ] {
            assert!(theme.style(role).add_modifier.contains(Modifier::BOLD));
        }
        assert!(
            theme
                .style(UiThemeRole::ConversationActivity)
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            theme
                .style(UiThemeRole::ConversationSelectionFocused)
                .add_modifier
                .contains(Modifier::REVERSED | Modifier::BOLD)
        );
    }

    #[test]
    fn semantic_role_replacement_is_independent() {
        let theme = UiTheme::terminal().with_style(
            UiThemeRole::FooterWarning,
            Style::new().fg(Color::Magenta).bg(Color::White),
        );
        assert_eq!(
            theme.style(UiThemeRole::FooterWarning).fg,
            Some(Color::Magenta)
        );
        assert_eq!(theme.style(UiThemeRole::Warning).fg, Some(Color::Yellow));
    }
}
