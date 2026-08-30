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
