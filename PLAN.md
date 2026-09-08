# HQ

## Next Up

### Conversation details as focused navigation

Problem: The runtime strip advertises D while the composer has focus, and runtime details replace the transcript without becoming a navigation destination; the collapsed composer still advertises Tab to compose underneath details.

Scope: In `crates/hq-tui/src/model.rs`, replace the loose `runtime_details` visibility flag with an appropriate typed navigation destination scoped to the selected conversation, integrating with `UiRoute`/`UiNavigation`. In `render.rs`, update `runtime_header_lines`, recovery details rendering, breadcrumbs, footer help, and collapsed-composer rendering.

- Show the D shortcut only when it can act on the focused conversation. Preserve D as ordinary draft input while composing; the current input dispatcher already gates runtime actions on conversation focus.
- Details should show a breadcrumb such as `HQ / Inbox / alice / Details`, with equivalent project paths. Esc returns to the same conversation and reading position. Do not advertise or enter the composer from the details destination; hide its collapsed prompt there.
- Preserve the exact conversation/runtime identity, recovery evidence and retry behavior, draft content, and transcript tail/anchor state. Clear stale details when the target conversation disappears or changes.

Completion: Model and rendering tests cover conversation versus composer focus, D typed into a draft, details breadcrumbs and back navigation, absence of composer hints in details, narrow layouts, and target changes. No daemon changes are required.

### Exit agent-specific setup back to the Inbox

Problem: Enter on an unassigned Inbox agent opens Choose project, but Esc exposes the generic New… launcher even though the user never entered that launcher.

Scope: Update agent-entry setup in `crates/hq-tui/src/model.rs`, especially `preferred_new_agent`, `UiNewWorkflow`, `apply_new_modal_input`, and navigation provenance. Extend `entering_an_unbound_inbox_agent_joins_project_setup_with_that_agent_selected` in `tests/model.rs`.

- Carry a typed entry origin and selected agent through the setup flow. For setup entered from an Inbox agent, Esc from Choose project exits the whole flow to the Inbox with that same agent selected.
- Treat the bound-agent setup as one user-facing flow rather than revealing a generic launcher on cancellation. Preserve ordinary back navigation for flows actually entered through New…, and preserve existing rules for work already submitted.
- Clear abandoned setup state so a later generic New… flow does not inherit the old agent. Ignore late asynchronous results from the cancelled flow rather than reopening it.

Completion: Tests demonstrate Inbox → unassigned agent → Choose project → Esc returns directly to the same Inbox selection; normal New… navigation still works, abandoned agent context does not leak, and delayed results do not resurrect cancelled setup.

### Word-aware composer wrapping

Problem: `draft_editor_layout`/`push_draft_span` in `crates/hq-tui/src/render.rs` wrap one grapheme at a time, splitting words at the right edge and displaying wrap-boundary spaces as indentation.

Scope: Make composer layout wrap whole words when they fit on a fresh display line. Hide separator whitespace at the beginning of a soft-wrapped continuation. Preserve explicit newlines and intentional indentation on hard lines. Words longer than the available width must still make progress by wrapping at grapheme boundaries.

- Keep draft bytes unchanged: soft wrapping and hidden whitespace are display-only. Sending, persistence, copy/edit operations, and markdown source must retain exact spaces and newlines.
- Preserve source-offset-to-display-position mapping, including a visible and predictable caret when it falls on hidden whitespace, at a wrap boundary, or at end of input.
- Use the same layout for painting, cursor visibility, composer height calculation, and transcript geometry. Preserve the reading anchor/tail behavior as the editor grows or the terminal resizes.
- Cover multiple separator spaces, tabs, empty lines, hard-line indentation, long tokens, combining characters, wide characters, joined emoji, and very narrow widths. Consider shared wrapping utilities from `message_markdown.rs` only where source-position semantics can be preserved.

Completion: Focused layout and render tests show words moving intact to the next row, no separator indentation on soft continuations, exact unchanged draft content, visible carets across wrap boundaries, and consistent composer/transcript geometry.
