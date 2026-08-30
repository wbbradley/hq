# HQ

## Product direction

Design the TUI for people who have never seen HQ and do not know its internal vocabulary. Every
screen and dialog must make clear what the user is looking at, why HQ needs their input, what they
can do next, and what will happen afterward. Prefer user intentions and ordinary language over
authority, reducer, provider-session, assignment, thread, reconciliation, and other implementation
terms. Preserve exact technical evidence behind contextual details and recovery views.

Keep these user workflows distinct and composable:

- Projects define work and authoritative ownership of resources. Resource ownership is a core HQ
  concern; Git worktree creation and lifecycle management are not the product's center and should
  remain optional, progressively disclosed conveniences. Agents may eventually manage worktrees
  themselves.
- Agents are named workers that can be assigned to project work and contacted through
  conversations. Starting work should hide routine provider-session and assignment mechanics.
- Direct messaging, including future communication with other humans in the HQ network, remains a
  first-class path rather than an awkward special case of project work.
- Personal notes remain available without competing with the primary collaboration actions.

Never require a user to guess a valid identifier, namespace, state transition, or recovery command
when HQ already has enough typed information to present valid choices. Use progressive disclosure:
ordinary screens explain goals and next actions; details screens expose stable IDs, causal evidence,
provider/session identities, and recovery diagnostics.

## Next Up

### Author and reconcile canonical project runtime events

Use the retained runtime-delivery evidence to make project output and activity canonically
project-bound without changing direct-agent runtime behavior.

- Preserve exact project provenance when a runtime turn is delivered. Carry the delivery operation's
  project, accepted input, dispatch, assignment binding, and selected project thread through the
  harness persistence boundary; author the existing canonical `ProjectOutputRecorded` fact for
  project-bound output instead of persisting it only as a project-less asynchronous message. Give
  project-bound activity the same typed project/dispatch association so it joins the project
  conversation rather than leaving an activity-only provider-session row. Make reconciliation
  idempotently repair existing attributable output without rewriting immutable history; conflicting
  or incomplete attribution must remain explicit instead of guessing.

Cover output/activity duplication, partial checkpoints, response loss, restart, late output after
handoff, and ambiguous bindings across application, harness, reducer, protocol, store, node, and
testkit layers. Update semantic-fact, payload-mapping, project-model, and storage documentation for
any appended canonical fact family or durable association.

### Group all project history under one typed conversation

- Add a cross-layer regression first for the exact reported corpus: two project inputs (`Let's have
  a conversation.` and `Let's have another conversation.`), the Codex response `Absolutely. What’s
  on your mind?`, and its completed activity currently render as two `Thread <hex>` rows plus one
  `codex · <provider-session>` row. Require one project conversation containing every input, output,
  and activity exactly once in canonical order, with no residual project-associated Thread or
  provider-session row. Prove the same grouping under shuffled arrival, rebuild, duplicate
  persistence, response loss, reconnect, handoff, and ambiguous concurrent bindings.
- Add a typed project conversation identity, such as `ConversationKey::Project { project_id }`,
  throughout the reducer/application query boundary, rebuildable store index and page query, local
  API DTOs/conversion, node client, and TUI model. Project-addressed human input must join that key
  immediately. Direct-agent and non-project conversations retain their own typed identities; never
  merge by content, display name, current assignment, provider/session coincidence, or row position.

### Make Inbox a human-readable, selection-driven master/detail workspace

- Separate stable conversation identity from row presentation. Extend the bounded conversation
  summary/read model with typed project and participant context plus a sanitized, clipped preview of
  the latest meaningful message's first nonblank line. Render human titles such as
  `Project name · Alice` for project work and `Me and Alice` for direct conversations, with the
  preview as secondary text. Do not use `Thread <hex>`, raw mailbox IDs, provider namespaces, or
  provider-session UUIDs as ordinary titles; retain exact values in technical details and use a
  plain unnamed-participant fallback when authoritative naming is unresolved.
- Make Inbox selection drive conversation loading. Reconciliation of the first nonempty Inbox
  snapshot must select a stable conversation and request its first page automatically; `j`/`k` or
  Up/Down in the Inbox list must immediately request the newly selected row. A newer selection may
  supersede an older pending preview, stale completions must be inert, rapid movement must not show
  the wrong page, and a completed preview must retain list focus rather than stealing conversation
  focus. Preserve the selected project conversation and logical message anchor across refresh,
  resize, reconnect, and authoritative reorder.
- Always render the Inbox conversation region, including bounded loading, empty, unavailable, and
  selected-diagnostic states; do not expand the list to full width while no page is loaded and do
  not require Enter to make the pane appear. Enter or `l`/Right moves focus into the already visible
  selected conversation. `h`/Left moves one hierarchical level at a time—drafting pane to
  conversation, conversation to Inbox list, and Inbox list to top-level navigation—rather than
  jumping from the conversation to `Inbox / Sent / Archived / Agents / Projects`. Apply the same
  back-to-list rule before section changes in compact layouts, and keep `j`/`k` scoped to the list
  or conversation that visibly owns focus.
- Render the wide Inbox as adjacent navigation, conversation-list, and conversation panes separated
  by single vertical dividers. Replace the conversation's `Block::bordered()` rectangle with an
  internal heading and a focused/unfocused left divider matching the navigation/list boundary; draw
  no top, bottom, or right box lines. Use one unboxed separator and an explicit back path in the
  compact stacked layout, preserving useful rows for messages instead of spending them on chrome.

Cover human labels and previews, eager selection loading, rapid stale-load replacement, stable
selection anchors, focus hierarchy, divider-only responsive layout, pure-model rendering, and
installed PTY navigation. Update `docs/rust/tui.md` so Inbox is documented as a persistent
master/detail workspace rather than a list that conditionally opens a modal-like rectangle.

### Move all message composition into the Inbox workspace

- Extend the durable draft vocabulary with a typed project target keyed by stable project ID and
  carry it exhaustively through `hq-tui`, the node TUI effect executor, local API v1, application
  planning, and store encoding. Resolve the project's current mailbox, account audience, and human
  authoring authority from transaction-consistent state; do not pretend a project message is direct
  messaging to whichever agent happens to be assigned.
- Separate the shared composer from modal presentation: replace `UiMailboxModal::Compose` with a
  modeless drafting pane owned by the Inbox/conversation workspace while retaining its durable
  behavior for replies, direct messages, notes, and project drafts. Recipient selection or a
  destructive confirmation may remain a bounded dialog, but message writing itself must not be.
  Preserve autosave, restart recovery, optimistic conflicts, bounded Unicode editing,
  save-on-Escape, and atomic draft consumption. A follow-up authored from an open project
  conversation must return to that same durable project conversation rather than creating a new
  user-facing thread row.
- In the guided `n New…` flow, submitting the first project message must continue to establish the
  exact activation thread and dispatch the retained input once, but completion must select and load
  the typed project conversation in `UiSection::Inbox`. It must not open a fresh
  `UiProjectModal::SendInput`. An ordinary subsequent project send must remain in the same Inbox
  conversation instead of mapping `InputSent` to `UiProjectCompletionContinuation::Details`.
  Remove `open_project_message_composer`, `ComposeInput`, every `UiProjectModal::SendInput` entry
  point, its form/rendering code, and `n send instructions` after all entry paths use the Inbox pane.
- While only one agent service is available, skip the `Start project work` confirmation dialog and
  proceed with that sole valid choice. Keep this conditional on the typed service catalog so the
  choice can return when it becomes meaningful.
- Keep the project-send command underneath for CLI and application compatibility, exact first-thread
  activation, automatic ordered pending-input dispatch, idempotency, and response-loss recovery.
  Keep failures beside the conversation/composer with the draft intact and a plain next action; a
  stale, archived, conflicted, or unassigned project must not silently retarget another mailbox,
  discard text, fall back to Project details, or expose routine dispatch as a normal user step.
- Cover durable project draft encoding/restart, autosave/submit races, duplicate completion,
  existing runnable projects, `conflict/project_activation_thread_missing`, and the complete
  post-bootstrap single-service path in store, local API, node mapping, pure-model, render, and
  installed PTY tests. Update `docs/rust/tui.md` and the acceptance walkthrough so the modeless
  Inbox workspace is the sole project-message experience and common writing surface for ordinary
  mailbox actions.

### Define the Projects workspace interaction model

Rethink Projects as a mostly modeless workspace for finding a project, understanding its status and
ownership, and continuing its work. Produce and review the interaction specification before
queuing implementation; do not turn the current `Project details` modal and its shortcut wall into
a differently styled modal without first agreeing on the product nouns, verbs, and boundaries.

- Inventory every datum and command currently packed into `UiProjectModal::Details`. Classify each
  as primary project work, ordinary administration, exceptional recovery, destructive lifecycle,
  or technical evidence, and record why it belongs in Projects, Inbox, contextual help, or nowhere
  in the ordinary UI.
- Define a novice-facing noun model centered on **project** (the durable work/ownership container),
  **conversation** (where the user and agent collaborate), **agent** or **worker** (who handles the
  project), and **folder** (the ordinary-language view of an owned resource). Keep assignment,
  thread, provider session, dispatch, operation, and claim as technical terms unless an exceptional
  decision or recovery genuinely requires one.
- Define the primary verbs before laying out controls. Selecting a project should expose a clear
  summary, and its primary action should open or continue the project conversation in Inbox.
  Project administration should use explicit object-bearing labels such as `Add folder`, `Change
  folder path`, `Remove folder`, `Use as working folder`, `Change assigned agent`, `Close project`,
  `Reopen project`, and `Archive project`; verify the exact wording against the underlying semantic
  effect rather than preserving `add`, `replace`, `primary`, or `move` by inertia.
- Produce wide and compact interaction maps for a modeless master/detail workspace. On wide
  terminals, project selection should update an adjacent detail/administration pane. On compact
  terminals, use ordinary navigable screens with a clear back path rather than an overlay that
  captures the application. Reserve modal confirmations for bounded destructive or force decisions,
  and keep forms, progress, routine outcomes, and technical inspection in their owning pane where
  possible.
- Replace the undifferentiated shortcut block with labeled, state-dependent controls or a
  discoverable action menu. For each current key (`a`, `e`, `x`, `p`, `r`/`R`, `n`, `v`, `d`, `h`,
  `c`/`o`, and `z`), decide whether to retain it as a secondary accelerator, rename it, move it to a
  relevant subview, expose it only during recovery, or remove it. Pending dispatch should normally
  be automatic and appear as a plainly named retry only when recovery is needed; folder health
  checks should live with folder status; `n send instructions` must disappear entirely; and local
  shortcuts must not obscure or conflict with global meanings.
- Walk the proposed model through fresh creation, an active conversation, an unassigned or blocked
  project, multiple folders, ownership conflict, agent handoff, pending-delivery recovery,
  close/reopen, archive/unarchive, and narrow-terminal use. Record a noun/verb glossary,
  state-dependent action matrix, terminal wireframes, focus/key behavior, and progressive-disclosure
  rules in `docs/rust/tui.md` (or a linked focused design note), including a migration path away from
  `UiProjectModal::Details`.
- Stop for user review of that interaction specification. After its nouns, primary action, layout,
  and contextual action matrix are agreed, add a separate front-of-queue implementation task with
  test-first model, responsive render, installed PTY, accessibility, documentation, and removal
  coverage; do not silently treat approval of this research task as approval of an invented UI.
