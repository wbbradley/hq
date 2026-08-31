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

### Implement the approved chat-like Inbox conversation surface

Implement the reviewed interaction contract in `docs/rust/inbox-conversation-surface.md` before
building the Projects workspace that navigates into it.

- Carry typed conversation display context through summary selection and page presentation so
  messages render as `You`, a resolved participant such as `Alice`, or an honest typed fallback.
  Replace the free-form entry summary with separate author/message/activity presentation; never
  recover identity by parsing row titles, purpose labels, command text, or shortened IDs.
- Add typed activity kind plus bounded human summary and exact detail to local API v1, reduction,
  persistence/projection, and TUI mapping as required. Keep protocol/schema version 1 in place
  without compatibility branches. Hide redundant successful turn completion only by typed kind;
  retain failures, interruptions, exact command output, and evidence.
- Replace the wide 60/40 split with one tested bounded-width function: Inbox list 24–36 columns,
  preferring 32, with at least 48 Conversation columns when available and every extra column going
  to Conversation. Preserve the always-visible compact stacked relationship and one-level Back.
- Render participant/context headings, column-zero author/body blocks, compact activity, actionable
  paging only, and modeless drafting without a Conversation box. Remove ordinary rendering of
  `Conversation · complete`, purpose/ID headings, `message · open`,
  `update · information only`, and renderer-added body indentation.
- Replace fixed entry-height capacity with display-cell-aware wrapped spans anchored by stable fact
  identity. Preserve paragraphs, wide Unicode, long content, selection across resize/reload/page
  load, and retained history on older-page failure.
- Add full-row transcript focus styles and semantic author/activity roles with explicit terminal,
  no-color, Base16, native-theme parsing, and documentation coverage. Color must supplement author
  labels, status symbols/text, reverse/weight focus, and other non-color cues.
- Move selected-entry technical disclosure into the approved in-pane inspector or compact secondary
  screen without overwriting the draft. Preserve complete routing, semantics, causal evidence,
  activity detail, exact IDs, and action targeting.
- Write failing protocol/projection/mapper and pure-model tests first, followed by measured-layout,
  style-aware wide/compact render snapshots, no-color/accessibility text, failure/paging/draft, and
  installed PTY coverage based on the reviewed Alice/project exchange. Update TUI, theme,
  acceptance-scenario, and behavior-ledger documentation, then remove obsolete presentation paths.

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
