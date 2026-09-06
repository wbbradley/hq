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

### Implementation plan

- Modify `crates/hq-tui/tests/model.rs` first:
  - Change list-selection tests to assert that materialized preview/cache updates leave the typed
    path at the Inbox root and that Enter installs the exact conversation route.
  - Cover Inbox, Sent, and Archived roots; one-level Escape; exact evidence push/pop; selection and
    viewport restoration; project-to-conversation canonical path installation; and same-workspace
    shortcut replacement.
  - Add refresh reorder/removal cases proving that active conversation/evidence identity is either
    re-resolved exactly or returned to the list root, never replaced by the row at the old index.
  - Extend stale/out-of-order completion, reconnect, draft, approval, and optimistic-entry tests
    with route assertions so an exact late operation cannot navigate a different active route.
- Modify `crates/hq-tui/src/model.rs`:
  - Make `UiRoute::Workspace`, `Conversation`, and `ConversationEvidence` the navigation source
    for Inbox-family list, transcript, and technical-detail depth.
  - Add small model helpers for entering an exact loaded or pending conversation, installing a
    canonical Inbox deep path, returning to the list while retaining its stable selection and
    viewport anchor, and pushing/popping exact transcript evidence.
  - Keep latest-value observation and retained first pages as caches outside navigation; moving a
    list selection may update observation interest but must not make the cached transcript active.
  - Correlate page/materialized-view application and automatic draft opening with the active exact
    route. A matching completion may populate caches while another route is active, but may not
    push, replace, or focus that route.
  - Preserve composer and conversation-scoped approval as adjacent focus only while their exact
    conversation route is active. Workspace/root replacement retains durable draft/approval data
    without letting it capture input.
  - On authoritative target removal, pop evidence/conversation routes to the appropriate list root
    and retain typed failure evidence where useful; never choose a new route from list position.
  - Retire conversation/evidence uses of `technical_visible` and pane-navigation focus where the
    typed path now expresses depth, leaving compatibility projections only where later tasks still
    need them.
- Modify `crates/hq-tui/src/render.rs`:
  - Render Inbox, Sent, and Archived workspace roots as full-content lists at every width.
  - Render an active conversation as the full content pane, with only its bounded composer or
    conversation-scoped approval sharing that rectangle.
  - Render exact message/activity evidence as a full-content pane at every width; remove Inbox
    master-detail splits, compact selected-summary preview, technical split, and pane-focus border
    cues.
  - Make Inbox-family footers describe only the active typed route and the result of Enter/Escape;
    retain minimum-size, semantic-theme, no-color, and display-cell safety.
- Modify `crates/hq-tui/tests/render_snapshots.rs` and the affected files under
  `crates/hq-tui/tests/snapshots/` to prove list, conversation, evidence, composer, and approval
  full-pane behavior at compact and wide sizes. Assert that non-entered cached content and unrelated
  list rows do not appear beside the active destination.

Risks: the materialized Inbox subscription intentionally delivers a selected first page alongside
the list, so the model must distinguish cached data from active route without losing eager loading
or unread reconciliation. Sent and Archived currently share row presentation but not the Inbox
latest-value subscription, so Enter may need the bounded page-load effect. Automatic follow-up and
project setup can open a draft only after installing their exact conversation route. Existing
tests and PTY snapshots encode master-detail presentation and must change only where the new
hierarchy requires it.

Acceptance: all Inbox-family visible depth is represented by the typed route path; list selection
and passive cache updates remain at a root; Enter and Escape change one meaningful level; exact
evidence is a full-pane child; drafts and scoped approvals are the only adjacent content; refresh,
reconnect, target removal, and late effects cannot steal navigation; compact and wide buffers show
the same hierarchy; strict linting and the full workspace suite pass.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/home/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/home/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

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
