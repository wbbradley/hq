# Conversation Markdown renderer

Status: accepted for implementation

Date: 2026-08-31

## Context

HQ conversation messages should render useful Markdown without giving participant-authored content
control of the terminal. The conversation view also measures entries before painting them, so the
eventual renderer must produce identical layout for measurement and display at the assigned width.
Draft editing remains raw Markdown, and activity entries remain typed non-Markdown presentation.

The node-to-TUI boundary owns a separate prerequisite: message bodies retain normalized line feeds
while tabs and terminal controls are neutralized. That boundary does not parse Markdown and does not
modify canonical message content. An HQ-owned renderer adapter will consume the safe presentation
string so dependency choice, theme policy, measurement, and resource behavior remain replaceable.

This decision compares compatible releases available on 2026-08-31. Registry metadata and the
published source were inspected for each candidate.

## Options

| Candidate | Compatibility and license | Output and layout | Theme and syntax | Dependency and side-effect boundary |
| --- | --- | --- | --- | --- |
| `tui-markdown` 0.3.9 | Uses `ratatui-core` 0.1 and tests against Ratatui 0.30; MIT OR Apache-2.0. Releases 0.3.8 and 0.3.9 were published in June and July 2026. | Returns Ratatui `Text`/`Line` values. It understands terminal display width inside tables and list markers, but accepts no target width and provides no measurement API. Ratatui performs later wrapping, while wide tables are emitted at their natural width. HQ must therefore own width-aware wrapping, clipping, and a single rendered artifact used for measurement and painting. | A `StyleSheet` trait covers semantic structures. It supports paragraphs, breaks, headings, emphasis, strong text, strikethrough, code, quotes, ordered/unordered/task lists, links, images, raw HTML, rules, and GFM tables, with some newer Markdown extensions explicitly unsupported. Links include visible destinations. | With default features disabled, rendering uses `pulldown-cmark`, `ratatui-core`, `itertools`, and `tracing`; the published crate also declares `pretty_assertions` and `rstest` as normal dependencies, so their test-support closure is present in production builds. The default `highlight-code` feature additionally brings `syntect` and `ansi-to-tui`, so HQ will disable it. Rendering produces text and styles only. Images have configurable text fallbacks and do not load resources. |
| `ratada::markdown` 0.5.0 | Uses Ratatui 0.30; MIT. The current 0.5.0 release and documentation were available at the decision date. | `render_block` accepts a display width and returns `Vec<Line<'static>>`; `measure_block` derives its count from the same renderer. It also provides inline clipping and a scrollable view. This is the strongest ready-made measurement API. | A concrete, comprehensive `StyleSheet` covers headings, emphasis, code, quotes, list markers, task boxes, tables, HTML, rules, and GFM callouts. It parses through `pulldown-cmark`. | The Markdown module itself is focused, but it is not independently packaged. Depending on `ratada` also brings its terminal driver, forms, modal/picker framework, `crossterm`, `chrono`, `nucleo-matcher`, logging, serialization, and Windows clipboard support. Link navigation is available in its viewer. Importing that application framework for one renderer would enlarge HQ's dependency and behavior boundary. |
| Focused `pulldown-cmark` 0.13.4 adapter | Parser is independent of Ratatui and compatible with HQ's versions; MIT. It is an established, actively maintained CommonMark parser. | Produces a borrowed event stream, not Ratatui text. HQ would own line construction, Unicode display widths, wrapping, indentation, tables, clipping, and exact height measurement. This offers full control but is a renderer subsystem rather than an adapter. | Parser options cover CommonMark plus GFM tables, task lists, strikethrough, footnotes, and other extensions. Every visual role and malformed-input fallback must be implemented and tested by HQ. | With default features disabled, the parser has a small, pure-Rust surface and performs no file, network, image, or terminal I/O. It is already the parser beneath both higher-level candidates. |

## Decision

Use `tui-markdown` 0.3.9 behind an HQ-owned message-body adapter, with default features disabled.
Do not add the dependency until the rendering capability is implemented. The adapter will supply an
HQ semantic stylesheet, force links and images to inert visible text, and expose only a rendered
artifact suitable for both height measurement and painting.

Adoption is conditional on focused narrow-layout gates. The implementation must prove that compact
messages, nested lists, long tokens, and especially tables cannot emit cells beyond the conversation
pane. It must build one width-specific artifact per message per render pass so measurement and paint
cannot diverge. If those constraints require substantial modification of `tui-markdown`, implement
the same adapter directly over `pulldown-cmark` instead. `ratada::markdown` remains useful design
evidence for width-aware APIs, but its package boundary is too broad for this use.

The adapter will not expose syntax-theme file loading, OSC-8 links, image protocols, link opening, or
other resource access. HQ will keep fenced code styling semantic and inert instead of enabling the
renderer's default highlighting feature.

The adapter maps Markdown onto the existing theme vocabulary: ordinary body text uses `ui.text`;
headings and table headers use `ui.heading`; links and list markers use `ui.accent`; inline and
fenced code use `ui.text.technical`; and quotes, raw HTML, image fallback text, and table borders use
`ui.text.muted`. Strong, emphasis, and strikethrough add structural modifiers. This mapping keeps
native, custom, and no-color themes complete without exposing renderer-specific configuration.

## Consequences

- Node presentation sanitization and Markdown rendering remain independent and testable.
- HQ accepts a small rendering dependency while retaining ownership of safety, theme, width,
  measurement, and fallback policy.
- Version 0.3.9 unnecessarily carries upstream test-support crates as normal dependencies. This is
  accepted for the initial adapter because it remains behaviorally inert, but a future release must
  remove that production dependency closure or HQ should reconsider the direct `pulldown-cmark`
  fallback.
- Width handling is the principal integration risk and must be resolved before ordinary message
  presentation changes.
- Maximum-page measurement showed parsing on every redraw was material. The installed terminal now
  owns a bounded 128-entry cache keyed by stable entry identity, exact content, width, and the
  semantic Markdown styles; the immutable model remains free of presentation cache state.
- A direct `pulldown-cmark` implementation remains a known fallback without changing callers.

## Sources

- [`tui-markdown` 0.3.9 crate and feature metadata](https://docs.rs/crate/tui-markdown/0.3.9)
- [`tui-markdown` API and resource behavior](https://docs.rs/tui-markdown/0.3.9/tui_markdown/)
- [`ratada::markdown` 0.5.0 API](https://docs.rs/ratada/0.5.0/ratada/markdown/)
- [`ratada` 0.5.0 crate surface](https://docs.rs/ratada/0.5.0/ratada/)
- [`pulldown-cmark` 0.13.4 crate metadata](https://docs.rs/crate/pulldown-cmark/0.13.4)
