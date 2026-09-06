# HQ

## Next Up

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

### Implementation plan

- Add model tests first for exact Agent, session, evidence, help, Config edit, New choice/form,
  provider request, confirmation, progress, outcome, and recovery paths, including one-level Back,
  stale completion, workspace replacement, and resize invariance.
- Project every remaining legacy interaction state onto one identity-bearing `UiRoute`, using agent,
  provider request, project, conversation, or global typed targets and the exact in-flight effect ID.
  Install agent deep links canonically and replace sequential workflow levels rather than retaining
  hidden dialog history.
- Make input dispatch and completion consume or restore route levels without allowing stale agent,
  session, configuration, mailbox, or provider-interaction results to alter an unrelated active
  route. Preserve all existing typed commands, editor state, draft durability, and force gates.
- Render New, mailbox decisions, agent administration, provider interactions, project workflows,
  Config editing, and help directly into the full content rectangle at compact and wide sizes;
  remove centered clears and background dialog presentation.
- Update renderer and installed-terminal coverage for the visible route transitions, then run
  formatting, strict workspace linting, focused route/render tests, and the full workspace suite.

Risks: the interaction enums still carry validated input and operation evidence, so they remain as
workflow data while navigation becomes the sole depth authority. Several flows can be nested under
different workspaces; canonical parent validation and exact target correlation must prevent stale
or incidental origin history from leaking across them.

Acceptance: every remaining dialog-like state has a matching typed active route, sequential state
uses replacement/consumption semantics, all such screens own the full content pane at every width,
and stale completions cannot steal another destination; strict linting and the full suite pass.

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
