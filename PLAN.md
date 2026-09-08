# HQ

## Next Up

### Word-aware composer wrapping

Problem: `draft_editor_layout`/`push_draft_span` in `crates/hq-tui/src/render.rs` wrap one grapheme at a time, splitting words at the right edge and displaying wrap-boundary spaces as indentation.

Scope: Make composer layout wrap whole words when they fit on a fresh display line. Hide separator whitespace at the beginning of a soft-wrapped continuation. Preserve explicit newlines and intentional indentation on hard lines. Words longer than the available width must still make progress by wrapping at grapheme boundaries.

- Keep draft bytes unchanged: soft wrapping and hidden whitespace are display-only. Sending, persistence, copy/edit operations, and markdown source must retain exact spaces and newlines.
- Preserve source-offset-to-display-position mapping, including a visible and predictable caret when it falls on hidden whitespace, at a wrap boundary, or at end of input.
- Use the same layout for painting, cursor visibility, composer height calculation, and transcript geometry. Preserve the reading anchor/tail behavior as the editor grows or the terminal resizes.
- Cover multiple separator spaces, tabs, empty lines, hard-line indentation, long tokens, combining characters, wide characters, joined emoji, and very narrow widths. Consider shared wrapping utilities from `message_markdown.rs` only where source-position semantics can be preserved.

Completion: Focused layout and render tests show words moving intact to the next row, no separator indentation on soft continuations, exact unchanged draft content, visible carets across wrap boundaries, and consistent composer/transcript geometry.
