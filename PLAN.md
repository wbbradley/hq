# HQ

## Next Up

## Establish the typed navigation controller and workspace roots

HQ's TUI has no canonical navigation representation. `UiSection`, `UiFocus`,
`UiProjectWorkspaceLevel`, and retained `UiSectionWorkspaces` currently combine to produce multiple
implicit histories. Establish one encapsulated typed route stack as the sole canonical path, and
make global workspace shortcuts replace it with the selected workspace root. Keep only stable list
selection state per workspace; switching workspaces must not restore cloned conversations, focus,
technical panes, or modal state. Resize must remain geometry-only.

This foundation must support push, pop, sequential replace/advance, completion to a typed return
level, canonical-path installation, and workspace-root replacement. Routes carry stable identity,
never display labels, timestamps, domain snapshots, or page-local indices. A closed route contract
must declare Enter, Escape, completion, and permitted adjacent UI semantics for each introduced
route family. Escape at a workspace root is a no-op, and `q`/Ctrl-C remains explicit exit.

### Implementation plan

- Modify `crates/hq-tui/src/model.rs`:
  - Add capability-named `UiRoute`, `UiWorkspace`, route-transition, return-level, and adjacent-UI
    types plus an encapsulated `UiNavigation` controller.
  - Implement checked stack algebra for `push`, `pop`, `replace`, `complete_to`, canonical path
    installation, and workspace switching. Reject paths whose root or parent/child relationship is
    not canonical.
  - Define the route contract table used to validate canonical parentage and expose Enter, Escape,
    completion, and adjacent-view behavior without deriving identity from labels.
  - Store `UiNavigation` in `UiModel`, derive the legacy `section()` projection from its workspace
    root during incremental migration, and remove `UiSectionWorkspaces`.
  - Replace per-workspace cloned state with a small stable-ID list-selection catalog. Workspace
    switching saves/re-resolves only the selected row, closes visible child state, installs the
    destination root, and cannot restore a draft/approval focus or conversation from a hidden
    history.
  - Keep asynchronous operation state, retained conversation pages, drafts, catalogs, and viewport
    evidence outside the route stack.
- Modify `crates/hq-tui/src/lib.rs` to export the public typed route and route-contract vocabulary
  required by renderers, shells, and integration tests.
- Modify `crates/hq-tui/tests/model.rs` first to add failing behavioral tests proving workspace-root
  replacement, stable selection re-resolution, absence of hidden restored detail/focus, Escape
  no-op at a root, and resize preservation of the exact route path.
- Add pure unit tests in `crates/hq-tui/src/model.rs` covering push/pop, replace/advance,
  completion-return truncation, invalid canonical paths, canonical deep-path installation, route
  contract edges, and non-cancellable progress behavior.

Risks: the existing model is intentionally incremental and many screens still use legacy fields;
the `section()` projection must remain temporarily available without becoming a second source of
navigation truth. Draft and pending-operation data must be retained while their input focus is
closed on workspace replacement. Later migration tasks may extend the closed route vocabulary,
but must use this controller rather than add parallel depth state.

Acceptance: pure stack tests pass; every introduced route has a declared contract; `UiModel` owns
one canonical path; shortcuts replace that path with exactly one workspace root; returning to a
workspace may restore an extant stable row selection but never a hidden conversation, modal,
technical pane, or input focus; and resize changes no route, selection identity, pending input, or
operation identity.

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
