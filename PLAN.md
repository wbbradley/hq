# HQ

## Next Up

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

### Implementation plan

- Replace the TUI architecture guide with the implemented navigation contract: one encapsulated
  typed path, destination-rooted deep links, full-pane route ownership, route-derived breadcrumbs
  and actions, sequential workflow replacement/completion, stable effect correlation, and the two
  conversation-scoped adjacent surfaces.
- Recast the Inbox and Projects guides as current route maps and canonical journeys. Remove their
  obsolete master/detail, pane-focus, responsive-hierarchy, centered-modal, and incidental-history
  rules while retaining stable selection, typed identities, causal evidence, durable input, project
  ownership, lifecycle safety, and recovery behavior.
- Reconcile the acceptance matrix and behavior ledger so their normative TUI requirements name
  typed paths, full-pane evidence/workflows, canonical Back and deep-link behavior, geometry-only
  resizing, and identity-correlated refresh/reconnect/effect handling.
- Strengthen shell and installed-pseudoterminal qualification where the canonical root, detail,
  workflow, completion, deep-link, reconnect, and one-level Back journeys are not already asserted;
  use visible breadcrumbs only as presentation evidence and typed model/effect assertions as the
  navigation authority.
- Run formatting, strict workspace linting, focused TUI model/render/shell/PTY tests, and the full
  workspace suite.

Risks: documentation can silently preserve contradictory interaction rules after the code is
correct, and terminal assertions can accidentally treat clipped labels as identity. Keep canonical
paths and effect identities normative, labels presentational, and compact/wide differences purely
geometric.

Acceptance: all five guides agree with the implemented route algebra and route-owned renderer; the
qualification suite exercises every requested journey without label-derived authority; strict
linting and the complete workspace test suite pass.

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
