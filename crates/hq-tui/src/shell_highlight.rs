//! Bounded renderer-owned Bash semantic highlighting.

use std::collections::{BTreeMap, VecDeque};

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

const CACHE_CAPACITY: usize = 64;
const CAPTURE_NAMES: [&str; 14] = [
    "comment",
    "constant",
    "function",
    "function.builtin",
    "keyword",
    "number",
    "operator",
    "property",
    "punctuation",
    "string",
    "string.special",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// Theme-neutral shell token class produced by the Bash grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShellTokenKind {
    Plain,
    Comment,
    Constant,
    Function,
    Keyword,
    Number,
    Operator,
    Punctuation,
    String,
    Variable,
}

/// One contiguous source fragment with one semantic class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShellSegment {
    pub text: String,
    pub kind: ShellTokenKind,
}

pub(crate) type HighlightedShell = Vec<Vec<ShellSegment>>;

/// Bounded semantic cache and reusable parser owned by one renderer.
pub(crate) struct ShellHighlightCache {
    configuration: Option<HighlightConfiguration>,
    highlighter: Highlighter,
    entries: BTreeMap<String, HighlightedShell>,
    order: VecDeque<String>,
}

impl Default for ShellHighlightCache {
    fn default() -> Self {
        let configuration = HighlightConfiguration::new(
            tree_sitter_bash::LANGUAGE.into(),
            "bash",
            tree_sitter_bash::HIGHLIGHT_QUERY,
            "",
            "",
        )
        .ok()
        .map(|mut configuration| {
            configuration.configure(&CAPTURE_NAMES);
            configuration
        });
        Self {
            configuration,
            highlighter: Highlighter::new(),
            entries: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl ShellHighlightCache {
    pub(crate) fn highlight(&mut self, key: &str, source: &str) -> &HighlightedShell {
        if !self.entries.contains_key(key) {
            let highlighted = self
                .highlight_source(source)
                .unwrap_or_else(|| plain_lines(source));
            if self.entries.len() == CACHE_CAPACITY
                && let Some(oldest) = self.order.pop_front()
            {
                self.entries.remove(&oldest);
            }
            self.entries.insert(key.to_owned(), highlighted);
            self.order.push_back(key.to_owned());
        }
        &self.entries[key]
    }

    fn highlight_source(&mut self, source: &str) -> Option<HighlightedShell> {
        let configuration = self.configuration.as_ref()?;
        let events = self
            .highlighter
            .highlight(configuration, source.as_bytes(), None, |_| None)
            .ok()?;
        let mut lines = vec![Vec::new()];
        let mut stack = Vec::new();
        for event in events {
            match event.ok()? {
                HighlightEvent::HighlightStart(highlight) => {
                    stack.push(capture_kind(highlight.0));
                }
                HighlightEvent::HighlightEnd => {
                    stack.pop()?;
                }
                HighlightEvent::Source { start, end } => {
                    let fragment = source.get(start..end)?;
                    push_fragment(
                        &mut lines,
                        fragment,
                        stack.last().copied().unwrap_or(ShellTokenKind::Plain),
                    );
                }
            }
        }
        Some(lines)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

fn capture_kind(index: usize) -> ShellTokenKind {
    match CAPTURE_NAMES.get(index).copied() {
        Some("comment") => ShellTokenKind::Comment,
        Some("constant") => ShellTokenKind::Constant,
        Some("function" | "function.builtin") => ShellTokenKind::Function,
        Some("keyword") => ShellTokenKind::Keyword,
        Some("number") => ShellTokenKind::Number,
        Some("operator") => ShellTokenKind::Operator,
        Some("property" | "variable" | "variable.builtin" | "variable.parameter") => {
            ShellTokenKind::Variable
        }
        Some("punctuation") => ShellTokenKind::Punctuation,
        Some("string" | "string.special") => ShellTokenKind::String,
        Some(_) | None => ShellTokenKind::Plain,
    }
}

fn push_fragment(lines: &mut HighlightedShell, fragment: &str, kind: ShellTokenKind) {
    for (index, part) in fragment.split('\n').enumerate() {
        if index > 0 {
            lines.push(Vec::new());
        }
        if !part.is_empty()
            && let Some(line) = lines.last_mut()
        {
            line.push(ShellSegment {
                text: part.to_owned(),
                kind,
            });
        }
    }
}

fn plain_lines(source: &str) -> HighlightedShell {
    source
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                Vec::new()
            } else {
                vec![ShellSegment {
                    text: line.to_owned(),
                    kind: ShellTokenKind::Plain,
                }]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{CACHE_CAPACITY, ShellHighlightCache, ShellTokenKind};

    #[test]
    fn bash_highlighting_retains_multiline_source_and_semantics() {
        let mut cache = ShellHighlightCache::default();
        let highlighted = cache.highlight("one", "if test \"$HOME\"; then\n  echo ok # done\nfi");

        assert_eq!(highlighted.len(), 3);
        assert_eq!(
            highlighted
                .iter()
                .flat_map(|line| line.iter())
                .map(|segment| segment.text.as_str())
                .collect::<String>(),
            "if test \"$HOME\"; then  echo ok # donefi"
        );
        assert!(highlighted.iter().flatten().any(|segment| {
            segment.kind == ShellTokenKind::Keyword && segment.text.contains("if")
        }));
        assert!(highlighted.iter().flatten().any(|segment| {
            segment.kind == ShellTokenKind::Comment && segment.text.contains("done")
        }));
    }

    #[test]
    fn cache_is_bounded_and_empty_source_has_one_line() {
        let mut cache = ShellHighlightCache::default();
        assert_eq!(cache.highlight("empty", "").len(), 1);
        for index in 0..=CACHE_CAPACITY {
            let key = format!("command-{index}");
            let source = format!("echo {index}");
            cache.highlight(&key, &source);
        }
        assert_eq!(cache.len(), CACHE_CAPACITY);
    }

    #[test]
    fn unavailable_parser_falls_back_to_exact_plain_source() {
        let mut cache = ShellHighlightCache {
            configuration: None,
            ..ShellHighlightCache::default()
        };

        assert_eq!(
            cache.highlight("fallback", "echo one\nprintf two"),
            &vec![
                vec![super::ShellSegment {
                    text: "echo one".to_owned(),
                    kind: ShellTokenKind::Plain,
                }],
                vec![super::ShellSegment {
                    text: "printf two".to_owned(),
                    kind: ShellTokenKind::Plain,
                }],
            ]
        );
    }
}
