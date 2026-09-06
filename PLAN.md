# HQ

## Next Up

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

### Implementation plan

- Extend project-focused model tests first to assert exact paths for list, summary, management,
  folders, folder details, evidence, forms, confirmations, progress, outcomes, and recovery.
  Cover Escape one level at a time, sequential replacement/completion, authoritative reorder and
  removal, stale operation completion, and compact/wide resize invariance.
- Replace `UiProjectWorkspaceLevel` as navigation authority with projections derived from
  `UiRoute`. Keep summary card/action/folder selection as stable local selection state, but install
  every visible depth through checked navigation operations carrying `project_id`, `folder_id`,
  capability, target, and effect identity.
- Make project selection Enter push the exact project route. Map summary cards to canonical Inbox
  conversation, Agents detail, management, folder, and project-evidence paths without parsing
  labels or retaining origin history. Missing exact targets must return to the Projects root or a
  typed recovery route rather than retargeting by list position.
- Bind project forms, confirmation, progress, outcome, and recovery screens to the existing typed
  project command and operation identities. Advance sequential screens by replacement and consume
  them to the exact project route after authoritative success; ignore stale completions for other
  active routes while retaining their operation evidence.
- Refactor project rendering so the Projects root, summary, management, folders, folder details,
  evidence, and workflow screens each own the full content rectangle at every width. Remove the
  project master/detail split and compact-only hierarchy while preserving semantic styling,
  terminal safety, and renderer purity.
- Update model, render-buffer, qualification, shell, and installed PTY tests to drive Enter and
  Escape according to the visible typed route. Preserve all existing creation, resource ownership,
  activation, handoff, lifecycle, reconciliation, and restart coverage.

Risks: project workflows currently combine visible depth across `project_workspace_level`, summary
card selection, several modal enums, and completion context. Migration must keep those data models
until their later route-family task while ensuring they no longer decide ordinary project depth.
Cross-workspace links must install destination-rooted paths atomically so Back never exposes the
incidental source workspace. Project command results can arrive after refresh or navigation and
must be matched by typed operation and target before changing the active route.

Acceptance: every ordinary Projects depth and project-owned workflow has one identity-bearing
route; list, summary, management, folders, details, and evidence are full-pane at compact and wide
sizes; canonical links install Inbox, Projects, or Agents destination roots; Escape and completion
follow the route contract; refresh/removal and stale effects cannot retarget or steal navigation;
strict linting and the full workspace suite pass.

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
