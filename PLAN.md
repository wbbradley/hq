# HQ

## Next Up

## Establish a typed full-pane TUI navigation stack

HQ's TUI currently expresses navigation through unrelated mechanisms: top-level `UiSection`,
pane-oriented `UiFocus`, `UiProjectWorkspaceLevel`, per-section workspace copies, and separate
mailbox, agent, project, New, help, and interaction modal state machines. `render_rows` also changes
the apparent hierarchy by terminal width: wide Inbox and Projects screens use master-detail panes,
compact Inbox retains an inactive preview pane, and dialogs cover centered rectangles. There is no
single definition of depth, Back, completion, breadcrumbs, or retained selection.

Replace those mechanisms with one navigation-controller-style stack describing the canonical path
from HQ's root to the current context. The content area normally renders only the active route at
full width and height. Enter pushes a genuinely nested destination or completes the active
sequential step; Escape pops one visible level; completing a child flow consumes its sequential
screens and returns to the declared prior level instead of leaving stale forms in history. Keep
only adjacent controls whose simultaneous context is essential: the message composer and a
conversation-scoped command approval may share the conversation screen. Do not retain the Inbox
list, Projects list, or an inactive dialog beside the active destination.

This is one active stack, not browser history and not one retained stack per workspace. Global
workspace shortcuts replace the stack with that workspace's root. Cross-workspace links deep-link
to the destination's canonical path: Project → conversation installs `HQ / Inbox / <conversation>`,
and a conversation's project link installs `HQ / Projects / <project>`. Escape follows the visible
canonical path rather than returning to whichever screen happened to contain the link. Workflows
can contribute implicit depth when a parent context remains meaningful, but sequential chooser,
form, review, progress, and outcome screens should replace or consume a workflow level whenever
revisiting the previous step would be misleading.

The stack must contain small typed route entries, not cloned `UiModel`, `UiSnapshot`, domain data,
rendered labels, or page-local indices. Identity-bearing routes retain the stable project, agent,
conversation, message/activity, folder, interaction, draft, effect, or operation identity needed
to re-resolve presentation from authoritative state. Display names and breadcrumbs are derived
presentation only. Async completions may update or navigate only the exact outstanding operation
and route identity; a late completion must not steal navigation after the user has left its route.
Invalidations remain body-free hints to reread authoritative state.

### Typed navigation foundation

- In `crates/hq-tui/src/model.rs`, introduce an encapsulated route stack with explicit operations
  for push, pop, replace/advance, complete to a typed return level, install a canonical deep path,
  and switch to a workspace root. Define capability-named route variants for workspace lists,
  selected summaries/details, conversations, technical evidence, help, choices, forms,
  confirmations, progress, outcomes, and recovery.
- Replace `UiFocus`, `UiProjectWorkspaceLevel`, `UiSectionWorkspaces`, and the competing `*_modal`
  and workflow depth conventions where routes express that state. Keep state that should outlive a
  screen—durable drafts, pending effects, retained conversation pages, viewport anchors,
  authoritative catalogs, connection state, and typed operation evidence—outside the stack and
  correlate it to routes by stable identity.
- Define a route table naming the canonical path, Enter result, Escape result, completion result,
  and permitted adjacent UI for every route. Include meaningful workflow-parent depth while
  marking sequential transitions that replace rather than push. Resize may change geometry only;
  it must not alter stack depth, selected identity, pending input, operations, or return levels.
- Cover stack algebra and every route-table edge in pure model tests. Include canonical deep links,
  workspace-root replacement, refresh reorder/removal, missing targets, stale and out-of-order
  async results, reconnect, resize, non-cancellable operations, and the invariant that no hidden
  route lies between two visible screens. Escape at a workspace root remains a no-op; `q`/Ctrl-C
  remains the explicit exit action.

### Full-pane chrome and rendering

- In `crates/hq-tui/src/render.rs`, make the active route the sole ordinary owner of the content
  rectangle at every supported width. Remove Inbox and Projects master-detail splits, the compact
  Inbox inactive-pane preview, pane-focus border logic, and centered modal overlays. Lists,
  conversations, summaries, details, forms, choosers, confirmations, progress, recovery, help, and
  technical evidence each render as normal full content panes.
- Turn the existing two-line header into route-aware chrome. Render a plain-language canonical path
  such as `HQ / Inbox / Alice` or `HQ / Projects / API redesign / Folders`, along with connection
  and refresh context. Define display-cell-safe clipping for narrow terminals that preserves the
  current destination and useful parent context. Breadcrumb strings must never become navigation
  identity.
- Make the footer explain the active route's available actions and their result. Remove pane-switch
  instructions when there is no adjacent pane. Preserve minimum-size handling, semantic themes,
  no-color meaning, and renderer purity.
- Add terminal-buffer coverage for every route family at compact and wide sizes, proving that width
  changes presentation within a route but never changes hierarchy, stack depth, or the active
  screen.

### Inbox and conversation navigation

- Make Inbox, Sent, and Archived full-pane list roots. Enter on an exact row pushes its full-pane
  conversation; Escape restores the same list selection and viewport. List selection may update
  the latest-value observation/cache, but a conversation is not rendered until entered. Preserve
  unread state, project filters, authoritative order, paging, optimistic entries, follow-tail,
  viewport anchors, automatic follow-up composition, and exact typed reply/archive eligibility.
- Keep message composition as the deliberate adjacent-view exception. A draft may occupy a bounded
  region beside or below its conversation; Escape saves/closes it to the same conversation, and a
  second Escape returns to the list. Conversation-scoped command approvals may use the same
  pattern; global prompts and permissions are full-pane routes. Changing workspace or installing a
  deep path must prevent retained drafts and approvals from capturing unrelated input.
- Push message/activity technical evidence as a full-pane detail route returning to the exact
  transcript item. If refresh removes that item, show a typed recovery state or pop to the
  conversation—never select a replacement by list position.
- Provide canonical bidirectional deep links between a project conversation and its project
  management context. Installing either link replaces the existing stack with the destination's
  canonical workspace path; Back stays within that visible path.

### Project navigation and administration

- Convert Projects to full-pane paths: project list → selected project summary → labeled management
  context → folder, agent, lifecycle, or technical destination. Folder and agent selection, forms,
  confirmations, progress, outcomes, and recovery retain exact identities. Sequential work
  advances/replaces its current workflow level, and successful completion returns to the refreshed
  selected object instead of exposing obsolete forms or outcomes on Back.
- Preserve the boundary that Projects owns work/resources, Inbox owns conversations and writing,
  and Agents owns independent worker/session management. Project links install the canonical Inbox
  or Agents path without parsing names, fabricating rows, or preserving incidental origin history.
- Preserve project creation, resource ownership checks, activation/handoff, close/archive/reopen,
  and uncertain-operation reconciliation evidence and idempotency while moving them to the shared
  route contract. Cover authoritative reorder/deletion/conflict and running, rejected, uncertain,
  and completed results.

### Agents, settings, creation, and decisions

- Convert Agents from list plus `UiAgentModal` into canonical full-pane list, exact agent detail,
  session/action chooser, confirmation, progress, and outcome paths. Preserve stable search and
  selection, provider/session correlation, conservative switch/retire behavior, and re-resolution
  of the same agent after refresh or completion.
- Convert Config editing, the global New launcher and its project/agent/direct-message/note
  branches, project-owned forms, mailbox recipient selection, help, and non-conversation
  interactions to the common route contract. Model genuine workflow parents as depth and replace
  sequential steps that should not remain reachable through Back.
- Render destructive/force decisions and global prompts as full-pane routes that capture all input
  and plainly state what Enter and Escape do. Preserve the shared Unicode-safe form editor and
  durable multiline draft editor, and keep projects, agents, direct messages, and personal notes as
  distinct data models despite their shared navigation primitives.
- Test that global shortcuts and ordinary text cannot leak through decisions or forms, and that a
  stale completion cannot close or replace a different active route.

### Documentation and end-to-end behavior

- Rewrite `docs/rust/tui.md` around the single typed stack, canonical deep paths, full-pane
  rendering, breadcrumbs, Back/completion rules, workflow depth, and conversation-adjacent
  exceptions. Retain the existing prohibition on stacks of cloned screens by distinguishing typed
  routes from copied application/domain state.
- Reconcile `docs/rust/inbox-conversation-surface.md` and `docs/rust/projects-workspace.md` with the
  new hierarchy. Remove requirements for persistent list/detail panes, compact previews, pane focus,
  and incidental cross-workspace return history while retaining typed identity, causal evidence,
  and ownership boundaries.
- Update `docs/rust/acceptance-scenarios.md` and `docs/rust/behavior-ledger.md` so acceptance covers
  canonical paths, full-pane parity across terminal sizes, meaningful workflow depth, consumed
  sequential screens, bidirectional canonical deep links, and the intentional adjacent exceptions.
- Update `crates/hq-node` shell and installed PTY coverage for representative root, detail,
  workflow, completion, deep-link, reconnect, and Back journeys. Assert typed state/effects and
  visible terminal output rather than relying on display labels as authority.

Acceptance criteria: every TUI screen has one typed canonical path or explicit workflow context;
the header communicates that path; ordinary content and former dialogs use the complete content
pane; Inbox and Projects no longer keep inactive master/detail panes visible; Enter pushes real
depth or advances/completes a sequential flow; Escape pops exactly one meaningful visible level;
workspace shortcuts and cross-workspace links install canonical destination-rooted paths; stale
forms and late operations cannot reappear or steal navigation; conversation composition and
conversation-scoped approval are the only documented adjacent exceptions; compact and wide
terminals have identical hierarchy; relevant pure, render, shell, and installed-terminal tests and
the full workspace suite pass.
