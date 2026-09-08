# HQ

## Next Up

### Capability-aware cancellation of agent work

Problem: The independent cancellation control now reaches the conversation model, but installed stop-and-continue behavior and stale provider-history handling still need qualification. Offer cancellation only for recipients with the capability; human recipients and unsupported harnesses must not offer or accept it.

Existing foundation: `docs/agent-cancellation.md` records the neutral target, daemon job registry, independently owned local API/TUI control workers, immutable request replay, and canonical terminal-state presentation. `HarnessSupervisor::cancel_owned` bypasses the ordinary worker lock; temporary submission backpressure retains saved input for authoritative acceptance lookup. Qualify the remaining behavior through these paths.

Scope and dependencies:

- Follow the completed control-path and ACP assessment in `docs/agent-cancellation.md`: implement neutral cancellation now, defer arbitrary ACP integration, and preserve the safe-submission registry gate.
- Extend the installed provider fixture in `crates/hq-node/tests/unix_tui_terminal.rs` to hold generation/tool execution until the actual interrupt arrives. Exercise Ctrl-G through the installed TUI/local API, verify exact provider thread/turn identity, persist and display authoritative interruption, then successfully send another message.
- Qualify blocked provider RPC handling and sibling responsiveness through the actual control path. Preserve the existing independent executor, server-session, responder-cleanup, and supervisor gate guarantees.
- Audit `CodexSession::read_thread`, `lookup_internal`, live lifecycle notifications, and `CodexOperationControl::activate`. A history response currently replaces `active_turn` and can reactivate an old operation after newer live evidence. Add deterministic late-history tests and preserve current-turn and terminal evidence without selecting authority from turn list position.
- Exercise natural completion races, duplicate/stale requests, uncertain acknowledgements, and cancellation while awaiting approval through the installed path. Preserve the clear distinction between requesting a stop and observing canonical terminal status, exact immutable retries, and draft/reading position.
- Verify that a future message sent while cancellation is pending survives temporary backpressure, retains its exact input identity, and proceeds afterward. Accepted work must be looked up rather than resubmitted, including across restart; project retries still require canonical eligibility.

Invariants: Scope cancellation to the authorized recipient/session/operation, never display prose. Unsupported recipients cannot invoke it. Preserve accepted-work recovery guarantees and sibling conversation responsiveness.

Completion: A deterministic integration test holds a turn/tool open and proves cancellation reaches the adapter before that work is released. Authoritative terminal state reaches the TUI, and the conversation can continue afterward. Cover natural completion races, duplicate/stale requests, uncertain outcomes, and cancellation while awaiting approval. Implementation must preserve the recovery guarantees identified in the completed ACP assessment.

Research anchors:

- [ACP v1 cancellation](https://agentclientprotocol.com/protocol/v1/prompt-turn#cancellation): session-scoped cancellation completes through the original prompt result and may permit final updates; assess the mismatch with HQ's operation-targeted contract and pending permission handling.
- [ACP initialization](https://agentclientprotocol.com/protocol/v1/initialization): examine baseline methods and negotiated optional capabilities.
- [Official ACP Rust SDK](https://agentclientprotocol.com/libraries/rust): ACP integration need not require TypeScript.
- `crates/hq-harness/src/registry.rs` requires provider submission idempotency or authoritative submission lookup. Determine whether the selected ACP version can satisfy this; session loading alone does not prove exact submission acceptance.

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
