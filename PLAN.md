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

### Complete the responsive chat-like Inbox conversation surface

Finish `docs/rust/inbox-conversation-surface.md` after typed voices and activity land.

- Replace the 60/40 wide split with the approved bounded 24–36-column Inbox list and dominant
  Conversation pane while preserving the always-visible compact stacked layout and one-level Back.
- Measure wrapped display-cell spans around the stable fact anchor instead of assuming fixed entry
  heights; cover paragraphs, wide Unicode, long content, paging, failure, resize, and an open draft.
- Add dedicated author/activity/full-row-focus semantic theme roles with terminal, no-color,
  Base16, native-theme, style-snapshot, and documentation coverage. Selection must not add a marker
  or shift the text origin.
- Move technical disclosure into the wide in-pane inspector or compact secondary screen without
  overwriting a draft; retain exact routing, semantics, evidence, activity detail, and action state.
- Finish participant/list hierarchy, actionable-only paging, loading/empty/failure states, compact
  behavior, installed PTY coverage, and removal assertions for `Conversation · complete`, all
  renderer-added transcript indentation, and obsolete presentation paths.
- Update TUI/theme/acceptance/behavior-ledger documentation and archive the approved umbrella
  implementation only after both stack layers pass the complete verification suite.

### Implement the approved Projects workspace

Replace `UiProjectModal::Details` with the modeless Projects workspace specified in
`docs/rust/projects-workspace.md`.

- Add typed, decoupled project summary, folder, assigned-agent, conversation-count, recovery, and
  technical-evidence presentation state. Preserve selection and focused objects by stable identity
  across reload, resize, stale completion, and asynchronous operation results.
- Implement persistent selection-driven list/detail panes on wide terminals and ordinary list,
  detail, management, and form screens with one-level Back on compact terminals. Keep forms,
  progress, normal results, and recoverable failures in their owning pane.
- Add typed zero/one/many project-conversation navigation into Inbox, including a visible clearable
  project filter. Keep all message composition in Inbox and never infer a canonical conversation
  from a display label, recency, assignment, or provider session.
- Implement labeled state-dependent project administration for folders, agent assignment,
  lifecycle, recovery, and technical details. Keep ordinary delivery automatic; expose retry only
  from typed stalled-delivery evidence. Use modal confirmation only for the approved bounded
  destructive or force decisions.
- Write failing pure-model and presentation tests first, then responsive render/snapshot,
  executor/local-client, no-color/accessibility-text, and installed PTY coverage for every current
  project-details command path and the specification's scenario matrix.
- Remove `UiProjectModal::Details`, its selected-resource state, shortcut wall and key branches,
  routine outcome dialogs, obsolete completion continuations, and all production rendering of
  `Project details`, only after replacement coverage passes.
- Update `docs/rust/tui.md`, `docs/rust/acceptance-scenarios.md`, and nearby architecture text to
  match the approved nouns, primary action, responsive layout, and progressive disclosure. Keep
  protocol/schema v1 shapes in place where changes are required; this pre-release work needs no
  backwards-compatibility layer or version bump.

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

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
