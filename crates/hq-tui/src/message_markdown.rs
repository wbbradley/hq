//! Inert, themed Markdown presentation for conversation message bodies.
//!
//! The adapter maps Markdown semantics onto HQ's existing theme vocabulary: ordinary body text uses
//! `Text`, headings and table headers use `Heading`, links and list markers use `Accent`, code uses
//! `TextTechnical`, and quotes plus table borders use `TextMuted`. Inline strong, emphasis, and
//! strikethrough retain their structural modifiers. Images and links are visible text only.

use std::{collections::VecDeque, sync::Arc};

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span, Text},
};
use tui_markdown::{AlertKind, ImageFallback, Options, StyleSheet, from_str_with_options};
use unicode_width::UnicodeWidthStr;

use crate::{UiTheme, UiThemeRole};

const MESSAGE_CACHE_CAPACITY: usize = 128;

/// Bounded terminal-owned cache for width- and theme-specific message artifacts.
#[derive(Default)]
pub(crate) struct MessageRenderCache {
    entries: VecDeque<CachedMessage>,
}

struct CachedMessage {
    entry_id: String,
    body: String,
    width: u16,
    styles: MessageStyleSheet,
    rendered: Arc<RenderedMessage>,
}

impl MessageRenderCache {
    pub(crate) fn render(
        &mut self,
        entry_id: &str,
        body: &str,
        width: u16,
        theme: &UiTheme,
    ) -> Arc<RenderedMessage> {
        let styles = MessageStyleSheet::from_theme(theme);
        if let Some(entry) = self.entries.iter().find(|entry| {
            entry.entry_id == entry_id
                && entry.width == width
                && entry.styles == styles
                && entry.body == body
        }) {
            return Arc::clone(&entry.rendered);
        }

        self.entries.retain(|entry| entry.entry_id != entry_id);
        let rendered = Arc::new(render_message_with_styles(body, width, styles));
        self.entries.push_back(CachedMessage {
            entry_id: entry_id.to_owned(),
            body: body.to_owned(),
            width,
            styles,
            rendered: Arc::clone(&rendered),
        });
        while self.entries.len() > MESSAGE_CACHE_CAPACITY {
            self.entries.pop_front();
        }
        rendered
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// One width-specific message artifact shared by measurement and painting.
pub(crate) struct RenderedMessage {
    text: Text<'static>,
    body_height: u16,
}

impl RenderedMessage {
    /// Returns the inert terminal text produced for the message.
    pub(crate) const fn text(&self) -> &Text<'static> {
        &self.text
    }

    /// Returns the exact wrapped body height at the artifact's width.
    pub(crate) const fn body_height(&self) -> u16 {
        self.body_height
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MessageStyleSheet {
    heading: Style,
    accent: Style,
    code: Style,
    muted: Style,
    text: Style,
}

impl MessageStyleSheet {
    fn from_theme(theme: &UiTheme) -> Self {
        Self {
            heading: theme.style(UiThemeRole::Heading),
            accent: theme.style(UiThemeRole::Accent),
            code: theme.style(UiThemeRole::TextTechnical),
            muted: theme.style(UiThemeRole::TextMuted),
            text: theme.style(UiThemeRole::Text),
        }
    }
}

impl StyleSheet for MessageStyleSheet {
    fn heading(&self, _level: u8) -> Style {
        self.heading
    }

    fn code(&self) -> Style {
        self.code
    }

    fn link(&self) -> Style {
        self.accent.add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        self.muted.add_modifier(Modifier::ITALIC)
    }

    fn heading_meta(&self) -> Style {
        self.muted
    }

    fn metadata_block(&self) -> Style {
        self.muted
    }

    fn html(&self) -> Style {
        self.muted
    }

    fn math_inline(&self) -> Style {
        self.code.add_modifier(Modifier::ITALIC)
    }

    fn math_display(&self) -> Style {
        self.code
    }

    fn footnote_ref(&self) -> Style {
        self.muted.add_modifier(Modifier::ITALIC)
    }

    fn footnote_def(&self) -> Style {
        self.muted
    }

    fn definition_term(&self) -> Style {
        self.heading
    }

    fn definition_description(&self) -> Style {
        self.text
    }

    fn alert(&self, _kind: AlertKind) -> Style {
        self.muted
    }

    fn alert_icon(&self, _kind: AlertKind) -> &'static str {
        ""
    }

    fn table_header(&self) -> Style {
        self.heading
    }

    fn table_cell(&self) -> Style {
        self.text
    }

    fn table_border(&self) -> Style {
        self.muted
    }

    fn image_alt(&self) -> Style {
        self.muted.add_modifier(Modifier::ITALIC)
    }
}

/// Builds the message artifact used for both layout and painting.
#[cfg(test)]
pub(crate) fn render_message(body: &str, width: u16, theme: &UiTheme) -> RenderedMessage {
    let styles = MessageStyleSheet::from_theme(theme);
    render_message_with_styles(body, width, styles)
}

fn render_message_with_styles(
    body: &str,
    width: u16,
    styles: MessageStyleSheet,
) -> RenderedMessage {
    let options = Options::new(styles).image_fallback(ImageFallback::AltTextAndUrl);
    let mut text = from_str_with_options(body, &options);
    let width = usize::from(width.max(1));
    normalize_text(&mut text, width, styles);
    let text = wrap_text(&text, width);
    let height = text.lines.len().max(1);
    RenderedMessage {
        text,
        body_height: u16::try_from(height).unwrap_or(u16::MAX),
    }
}

#[derive(Clone)]
struct OwnedGrapheme {
    symbol: String,
    style: Style,
    width: usize,
    whitespace: bool,
}

fn wrap_text(text: &Text<'_>, width: usize) -> Text<'static> {
    let lines = text
        .lines
        .iter()
        .flat_map(|line| wrap_line(line, width))
        .collect::<Vec<_>>();
    Text::from(lines)
}

fn wrap_line(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    let first_content = line.spans.iter().find(|span| !span.content.is_empty());
    let continuation_width = first_content
        .filter(|span| is_list_marker(span.content.as_ref()))
        .map_or(0, Span::width)
        .min(width.saturating_sub(1));
    let continuation_style = first_content.map_or(line.style, |span| line.style.patch(span.style));
    let graphemes = line
        .styled_graphemes(Style::new())
        .filter_map(|grapheme| {
            let grapheme_width = UnicodeWidthStr::width(grapheme.symbol);
            (grapheme_width <= width).then(|| OwnedGrapheme {
                symbol: grapheme.symbol.to_owned(),
                style: grapheme.style,
                width: grapheme_width,
                whitespace: grapheme.symbol.chars().all(char::is_whitespace),
            })
        })
        .collect::<Vec<_>>();
    if graphemes.is_empty() {
        return vec![Line::default()];
    }

    let mut tokens: Vec<Vec<OwnedGrapheme>> = Vec::new();
    for grapheme in graphemes {
        if tokens.last().is_some_and(|token| {
            token
                .first()
                .is_some_and(|first| first.whitespace == grapheme.whitespace)
        }) {
            if let Some(token) = tokens.last_mut() {
                token.push(grapheme);
            }
        } else {
            tokens.push(vec![grapheme]);
        }
    }

    let prefix = (continuation_width > 0).then(|| OwnedGrapheme {
        symbol: " ".repeat(continuation_width),
        style: continuation_style,
        width: continuation_width,
        whitespace: true,
    });
    let mut wrapped = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0_usize;
    let mut pending_whitespace = Vec::new();
    for token in tokens {
        if token.first().is_some_and(|grapheme| grapheme.whitespace) {
            pending_whitespace = token;
            continue;
        }

        let whitespace_width = grapheme_width(&pending_whitespace);
        let token_width = grapheme_width(&token);
        if current_width
            .saturating_add(whitespace_width)
            .saturating_add(token_width)
            <= width
        {
            current.append(&mut pending_whitespace);
            current_width = current_width.saturating_add(whitespace_width);
            current.extend(token);
            current_width = current_width.saturating_add(token_width);
            continue;
        }

        if token_width <= width.saturating_sub(continuation_width) {
            push_wrapped_line(&mut wrapped, &mut current, &mut current_width);
            start_continuation(&mut current, &mut current_width, prefix.as_ref());
            current.extend(token);
            current_width = current_width.saturating_add(token_width);
            pending_whitespace.clear();
            continue;
        }

        pending_whitespace.clear();
        for grapheme in token {
            if current_width.saturating_add(grapheme.width) > width {
                push_wrapped_line(&mut wrapped, &mut current, &mut current_width);
                start_continuation(&mut current, &mut current_width, prefix.as_ref());
            }
            current_width = current_width.saturating_add(grapheme.width);
            current.push(grapheme);
        }
    }
    if current_width.saturating_add(grapheme_width(&pending_whitespace)) <= width {
        current.append(&mut pending_whitespace);
    }
    push_wrapped_line(&mut wrapped, &mut current, &mut current_width);
    if wrapped.is_empty() {
        wrapped.push(Line::default());
    }
    wrapped
}

fn grapheme_width(graphemes: &[OwnedGrapheme]) -> usize {
    graphemes.iter().map(|grapheme| grapheme.width).sum()
}

fn start_continuation(
    current: &mut Vec<OwnedGrapheme>,
    current_width: &mut usize,
    prefix: Option<&OwnedGrapheme>,
) {
    if let Some(prefix) = prefix {
        current.push(prefix.clone());
        *current_width = prefix.width;
    }
}

fn push_wrapped_line(
    wrapped: &mut Vec<Line<'static>>,
    current: &mut Vec<OwnedGrapheme>,
    current_width: &mut usize,
) {
    if current.is_empty() {
        return;
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for grapheme in current.drain(..) {
        if let Some(span) = spans.last_mut().filter(|span| span.style == grapheme.style) {
            span.content.to_mut().push_str(&grapheme.symbol);
        } else {
            spans.push(Span::styled(grapheme.symbol, grapheme.style));
        }
    }
    wrapped.push(Line::from(spans));
    *current_width = 0;
}

fn normalize_text(text: &mut Text<'_>, width: usize, styles: MessageStyleSheet) {
    for line in &mut text.lines {
        for span in &mut line.spans {
            if span.content.chars().any(char::is_control) {
                span.content = span
                    .content
                    .chars()
                    .map(|character| {
                        if character.is_control() {
                            ' '
                        } else {
                            character
                        }
                    })
                    .collect::<String>()
                    .into();
            }
        }
        if let Some(marker) = line
            .spans
            .iter_mut()
            .find(|span| !span.content.is_empty() && is_list_marker(span.content.as_ref()))
        {
            marker.style = styles
                .accent
                .add_modifier(marker.style.add_modifier)
                .remove_modifier(marker.style.sub_modifier);
        }
        if is_table_line(line) {
            clip_line(line, width, styles.muted);
        }
    }
}

fn is_list_marker(value: &str) -> bool {
    let marker = value.trim_start();
    if matches!(marker, "- " | "- [ ] " | "- [x] ") {
        return true;
    }
    marker.strip_suffix(". ").is_some_and(|number| {
        !number.is_empty() && number.chars().all(|character| character.is_ascii_digit())
    })
}

fn is_table_line(line: &Line<'_>) -> bool {
    line.spans.iter().any(|span| {
        span.content.chars().any(|character| {
            matches!(
                character,
                '┌' | '┬' | '┐' | '├' | '┼' | '┤' | '└' | '┴' | '┘' | '│'
            )
        })
    })
}

fn clip_line(line: &mut Line<'_>, width: usize, ellipsis_style: Style) {
    if line.width() <= width {
        return;
    }
    if width == 0 {
        line.spans.clear();
        return;
    }

    let content_width = width - 1;
    let graphemes = line
        .styled_graphemes(Style::new())
        .map(|grapheme| (grapheme.symbol.to_owned(), grapheme.style))
        .collect::<Vec<_>>();
    let mut clipped: Vec<Span<'static>> = Vec::new();
    let mut used = 0_usize;
    for (symbol, style) in graphemes {
        let symbol_width = UnicodeWidthStr::width(symbol.as_str());
        if used.saturating_add(symbol_width) > content_width {
            break;
        }
        if let Some(span) = clipped.last_mut().filter(|span| span.style == style) {
            span.content.to_mut().push_str(&symbol);
        } else {
            clipped.push(Span::styled(symbol, style));
        }
        used = used.saturating_add(symbol_width);
        if used == content_width {
            break;
        }
    }
    clipped.push(Span::styled("…", ellipsis_style));
    line.spans = clipped;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ratatui::style::{Color, Modifier, Style};
    use unicode_width::UnicodeWidthStr;

    use super::{MESSAGE_CACHE_CAPACITY, MessageRenderCache, render_message};
    use crate::{UiTheme, UiThemeRole};

    #[test]
    fn renders_core_inline_and_block_markdown_with_structure() {
        let rendered = render_message(
            "# Heading\n\n**strong** *emphasis* ~~removed~~ and `code`",
            80,
            &UiTheme::terminal(),
        );

        assert_eq!(
            rendered.text().to_string(),
            "# Heading\n\nstrong emphasis removed and code"
        );
        let spans = rendered
            .text()
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .collect::<Vec<_>>();
        assert!(spans.iter().any(|span| {
            span.content == "strong" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        assert!(spans.iter().any(|span| {
            span.content == "emphasis" && span.style.add_modifier.contains(Modifier::ITALIC)
        }));
        assert!(spans.iter().any(|span| {
            span.content == "removed" && span.style.add_modifier.contains(Modifier::CROSSED_OUT)
        }));
    }

    #[test]
    fn renders_lists_quotes_code_links_images_and_breaks_as_inert_text() {
        let rendered = render_message(
            concat!(
                "soft\nbreak  \nhard\n\n",
                "> quoted\n\n",
                "1. first\n   - nested item\n- [x] done\n\n",
                "```rust\nlet value = 1;\n```\n\n",
                "[docs](https://example.test) ![diagram](file:///tmp/private.png)",
            ),
            80,
            &UiTheme::terminal(),
        );
        let text = rendered.text().to_string();

        assert!(text.contains("soft break\nhard"), "{text}");
        assert!(text.contains("> quoted"), "{text}");
        assert!(text.contains("1. first"), "{text}");
        assert!(text.contains("    - nested item"), "{text}");
        assert!(text.contains("- [x] done"), "{text}");
        assert!(text.contains("```rust\nlet value = 1;\n```"), "{text}");
        assert!(text.contains("docs (https://example.test)"), "{text}");
        assert!(
            text.contains("[img] diagram (file:///tmp/private.png)"),
            "{text}"
        );
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn bounds_tables_at_narrow_width_and_preserves_them_at_wide_width() {
        let source = "| Name | Description |\n| --- | --- |\n| alpha | a very long table value |";
        let narrow = render_message(source, 14, &UiTheme::terminal());
        let wide = render_message(source, 80, &UiTheme::terminal());

        assert!(narrow.text().to_string().contains('…'));
        assert!(narrow.text().lines.iter().all(|line| line.width() <= 14));
        assert!(wide.text().to_string().contains("a very long table value"));
        assert!(wide.text().lines.iter().all(|line| line.width() <= 80));
    }

    #[test]
    fn narrow_nested_lists_indent_every_continuation_line() {
        let rendered = render_message(
            "- outer\n    - nested item with enough words to wrap twice",
            18,
            &UiTheme::terminal(),
        );
        let lines = rendered
            .text()
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        assert!(lines.iter().all(|line| line.width() <= 18), "{lines:?}");
        let nested = lines
            .iter()
            .position(|line| line.starts_with("    - nested"));
        assert!(nested.is_some(), "{lines:?}");
        let nested = nested.unwrap_or_default();
        assert!(
            lines
                .iter()
                .skip(nested + 1)
                .all(|line| line.starts_with("      ")),
            "{lines:?}"
        );
        assert_eq!(
            rendered.body_height(),
            u16::try_from(lines.len()).unwrap_or(u16::MAX)
        );
    }

    #[test]
    fn malformed_unicode_html_long_tokens_and_controls_remain_safe_and_readable() {
        let rendered = render_message(
            "**open [link]( e\u{301} 👩‍💻 界 <b>literal</b> abcdefghijklmnop\x1b\u{0085}\u{007f}",
            8,
            &UiTheme::terminal(),
        );
        let text = rendered.text().to_string();
        let compact = text.replace('\n', "");

        assert!(text.contains("e\u{301} 👩‍💻 界"), "{text}");
        assert!(compact.contains("<b>literal</b>"), "{text}");
        assert!(compact.contains("abcdefghijklmnop"), "{text}");
        assert!(
            text.chars()
                .all(|character| !character.is_control() || character == '\n')
        );
        assert!(rendered.body_height() > 1);
        assert_eq!(render_message("", 8, &UiTheme::terminal()).body_height(), 1);
    }

    #[test]
    fn semantic_styles_follow_custom_and_no_color_themes() {
        let custom = UiTheme::terminal()
            .with_style(UiThemeRole::Heading, Style::new().fg(Color::Magenta))
            .with_style(UiThemeRole::Accent, Style::new().fg(Color::Green))
            .with_style(UiThemeRole::TextTechnical, Style::new().fg(Color::Yellow))
            .with_style(UiThemeRole::TextMuted, Style::new().fg(Color::Blue));
        let rendered = render_message(
            "# head\n\n1. item\n\n[link](https://example.test) `code`\n\n> quote",
            80,
            &custom,
        );
        let spans = rendered
            .text()
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .collect::<Vec<_>>();
        assert!(spans.iter().any(|span| {
            span.content.contains("# head") && span.style.fg == Some(Color::Magenta)
        }));
        assert!(
            spans
                .iter()
                .any(|span| span.content == "1. " && span.style.fg == Some(Color::Green))
        );
        assert!(
            spans
                .iter()
                .any(|span| span.content == "code" && span.style.fg == Some(Color::Yellow))
        );
        assert!(spans.iter().any(|span| span.style.fg == Some(Color::Blue)));

        let no_color = render_message("# head\n\n[link](url)\n\n> quote", 40, &UiTheme::no_color());
        for line in &no_color.text().lines {
            assert!(line.style.fg.is_none());
            for span in &line.spans {
                assert!(span.style.fg.is_none());
                assert_eq!(UnicodeWidthStr::width(span.content.as_ref()), span.width());
            }
        }
    }

    #[test]
    fn cache_keys_every_presentation_input_and_remains_bounded() {
        let terminal = UiTheme::terminal();
        let no_color = UiTheme::no_color();
        let mut cache = MessageRenderCache::default();

        let original = cache.render("entry", "**body**", 40, &terminal);
        let hit = cache.render("entry", "**body**", 40, &terminal);
        assert!(Arc::ptr_eq(&original, &hit));

        let changed_body = cache.render("entry", "*body*", 40, &terminal);
        assert!(!Arc::ptr_eq(&hit, &changed_body));
        let changed_width = cache.render("entry", "*body*", 20, &terminal);
        assert!(!Arc::ptr_eq(&changed_body, &changed_width));
        let changed_theme = cache.render("entry", "*body*", 20, &no_color);
        assert!(!Arc::ptr_eq(&changed_width, &changed_theme));

        for index in 0..=MESSAGE_CACHE_CAPACITY {
            let _ = cache.render(&format!("entry-{index}"), "body", 40, &terminal);
        }
        assert_eq!(cache.len(), MESSAGE_CACHE_CAPACITY);
    }
}
