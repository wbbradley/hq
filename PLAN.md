# HQ

## Next Up

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

### Implementation plan

- Add renderer tests first for route-aware breadcrumbs and active-route footers across workspace,
  exact detail/evidence, workflow, progress/outcome/recovery, compact, wide, no-color, and minimum
  terminal sizes; assert rendering and resizing never mutate the route path or input state.
- Introduce a pure presentation projection from the typed navigation path to plain-language route
  labels, resolving current project/agent/conversation names only from authoritative model state and
  clipping by display cells while retaining the active destination and useful root context.
- Replace the section-only header with the breadcrumb projection plus connection/refresh status.
  Make footer action text dispatch on the active route contract and interaction state rather than
  hidden pane focus or legacy modal geometry.
- Remove remaining pane-focus border cues, inactive preview wording, centered-overlay assumptions,
  and non-conversation adjacent layout. Keep composer and exact conversation approval adjacency,
  semantic themes, terminal-safe text, and renderer purity.
- Run formatting, strict workspace linting, focused model/render and terminal tests, then the full
  workspace suite.

Risks: labels are presentation only and can disappear or change after refresh, so they must never
feed navigation. Narrow clipping must preserve Unicode scalar boundaries and terminal cell width,
and must favor the current destination without making the root or connection state misleading.

Acceptance: the active typed route alone selects the content surface and footer actions;
breadcrumbs communicate root-to-current context safely at every supported width; only conversation
input shares content; resize is geometry-only; strict linting and the full suite pass.

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
