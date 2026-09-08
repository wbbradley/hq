# HQ

## Next Up

### Capability-aware cancellation of agent work

Problem: The conversation view cannot stop a running agent turn. Offer cancellation only for recipients with the capability; human recipients and unsupported harnesses must not offer or accept it.

Existing foundation: `crates/hq-harness/src/contract.rs` defines `HarnessCapability::OperationCancellation`, `HarnessSession::cancel_operation`, and cancellation outcomes. `HarnessSupervisor::cancel` in `supervisor.rs` routes through the live session. `crates/hq-codex/src/adapter.rs` maps an exact HQ operation to Codex `turn/interrupt` with provider thread and turn IDs. Extend these neutral abstractions rather than introducing a Codex-specific UI verb.

Scope and dependencies:

- Follow the completed control-path and ACP assessment in `docs/agent-cancellation.md`: implement neutral cancellation now, defer arbitrary ACP integration, and preserve the safe-submission registry gate.
- Expose typed recipient capabilities and an exact active cancellation target through application ports, the local API, node composition, TUI client, and model. Derive availability from authoritative capabilities and current operation state; revalidate authorization and exact scope in the daemon.
- Add a discoverable conversation/composer action with clear request/result states. Cancellation is a control operation, distinct from ordinary message content, question-thread cancellation, and rejecting a pending approval.
- Trace dispatch and lock ownership through `crates/hq-node/src/tui_client.rs`, `crates/hq-local-api/src/server.rs`, `crates/hq-node/src/harness_component.rs`, and the supervisor. Ensure cancellation reaches the adapter while generation or a tool is running. The supervisor currently holds its worker mutex during cancellation RPC; prevent long work, blocked submission/response handling, or other sessions from indefinitely serializing the control path.
- Distinguish an interrupt request from authoritative terminal cancellation. Audit the Codex adapter's immediate Cancelled result after the interrupt RPC succeeds. Handle completion races, duplicates, stale targets, transport uncertainty, and pending interactive requests without stopping a newer turn or reviving cancelled work.

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
