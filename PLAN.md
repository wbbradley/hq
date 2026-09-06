# HQ

## Next Up

## Migrate Inbox lists, conversations, evidence, and adjacent composition

Move Inbox, Sent, and Archived onto full-pane list and conversation routes. Enter pushes the exact
conversation, Escape restores the same stable list selection and viewport, and technical evidence
is a full-pane child bound to the exact transcript item. Preserve authoritative paging/order,
unread state, optimistic entries, follow-tail, automatic follow-up composition, project filters,
and typed reply/archive eligibility. Keep the composer and conversation-scoped approval as the only
adjacent exceptions; workspace changes and deep links must prevent retained input from capturing
unrelated keys. Missing refreshed targets produce typed recovery or pop, never positional retargeting.

Acceptance: Inbox-family content is full-pane at all widths; selection alone never renders a
conversation; Back follows the visible path; refresh/reorder/removal and stale/out-of-order
conversation operations remain identity-safe; and compact/wide model and buffer tests prove the
same hierarchy.

## Migrate Projects and cross-workspace canonical links

Move Projects onto project-list, exact summary, management, folders, agent/lifecycle, technical,
form, confirmation, progress, outcome, and recovery routes. Sequential workflow screens replace or
consume their level, and successful completion returns to the refreshed exact object. Preserve
creation, ownership checks, assignment/handoff, close/archive/reopen, uncertain reconciliation,
idempotency, and stable operation evidence.

Implement canonical bidirectional links: Project to conversation installs `HQ / Inbox /
<conversation>` and conversation to project installs `HQ / Projects / <project>`. Agent links
likewise install the canonical Agents path. No link may parse a label, fabricate a row, or preserve
incidental origin history.

Acceptance: Projects has no master/detail split at any width; every action and completion follows
the route contract; authoritative reorder/deletion/conflict and running/rejected/uncertain/completed
results remain correlated by typed identities; deep-link Back behavior stays within the installed
canonical path.

## Migrate Agents, Config, New, help, and global decisions

Replace `UiAgentModal`, `UiNewModal`, global interaction modals, Config editing depth, and help
overlay depth with full-pane routes. Cover exact agent detail, service/session choices,
confirmations, progress/outcomes, the New launcher's distinct project/agent/direct-message/note
branches, project-owned forms, recipient selection, help, permissions, destructive/force choices,
and recovery. Preserve stable search/selection, provider/session correlation, conservative
switch/retire behavior, Unicode-safe editors, durable drafts, and distinct project, agent, direct
message, and note models.

Acceptance: global decisions and forms capture all input; sequential screens are consumed; stale
completions cannot close or replace another route; all former dialogs render in the full content
pane; and route-family model/render tests cover compact and wide terminals.

## Render route-aware full-pane chrome

Make the active route the sole ordinary owner of the content rectangle. Remove Inbox and Projects
master-detail layouts, compact inactive previews, pane-focus borders, and centered modal overlays.
Render route-aware plain-language breadcrumbs with connection/refresh context and display-cell-safe
clipping that preserves the current destination and useful parent context. Derive labels only from
current authoritative presentation; breadcrumbs are never identity. Make the footer explain only
the active route's actions and their results while preserving minimum-size behavior, semantic
themes, no-color meaning, and renderer purity.

Acceptance: terminal-buffer tests cover every route family at compact and wide sizes and prove
width changes only geometry, never hierarchy, stack depth, active route, pending input, or return
levels. Conversation composition and conversation-scoped approval are the only documented adjacent
views.

## Document and qualify canonical TUI journeys

Rewrite `docs/rust/tui.md` around the typed stack, canonical paths, full-pane rendering,
breadcrumbs, Back/completion rules, workflow depth, and adjacent conversation exceptions. Reconcile
`docs/rust/inbox-conversation-surface.md`, `docs/rust/projects-workspace.md`,
`docs/rust/acceptance-scenarios.md`, and `docs/rust/behavior-ledger.md`, removing persistent
list/detail, compact preview, pane focus, and incidental return-history requirements while retaining
typed identity, causal evidence, and ownership boundaries.

Update `crates/hq-node` shell and installed PTY tests for representative root, detail, workflow,
completion, canonical deep-link, reconnect, and Back journeys. Assert typed state/effects and
visible output, not display labels as authority. Run formatting, strict linting, all relevant model,
render, shell, and installed-terminal tests, then the full workspace suite.

Acceptance: every TUI screen has one typed canonical path or explicit workflow context; the header
communicates it; Enter pushes meaningful depth or advances/completes a sequential flow; Escape pops
one visible level; global shortcuts/deep links install canonical destination-rooted paths; stale
screens and operations cannot reappear or steal navigation; compact and wide terminals have
identical hierarchy; documentation matches behavior; and the full workspace suite passes.
