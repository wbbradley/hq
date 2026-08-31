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

#### Implementation plan

1. Audit the current implementation in `crates/hq-tui/src/model.rs`,
   `crates/hq-tui/src/render.rs`, `crates/hq-node/src/tui_client.rs`,
   `crates/hq-node/src/local_client.rs`, `crates/hq-application/src/project.rs`, and
   `crates/hq-projects/src/workflow.rs`. Record every field carried by `UiProject`, every datum
   rendered by `UiProjectModal::Details`, every details key path, its state gate, and the actual
   application or inspection effect. Treat reducer and workflow semantics as authoritative when
   current labels disagree with behavior.
2. Create `docs/rust/projects-workspace.md` as the reviewable product specification. Define the
   novice-facing project, conversation, agent, and folder nouns; explicitly reserve assignment,
   thread, provider session, dispatch, operation, claim, head, and resource identity for technical
   disclosure. State what Projects owns, what Inbox owns, and what belongs only in contextual help
   or recovery.
3. In that specification, classify all current detail data and commands as primary work, ordinary
   administration, exceptional recovery, destructive lifecycle, or technical evidence. Define
   exact object-bearing labels and state-dependent availability for opening/continuing the project
   conversation, adding/changing/removing/selecting/checking folders, assigning/changing an agent,
   retrying delivery, closing/reopening, and archiving/restoring. Include an explicit disposition
   for every current `a`, `e`, `x`, `p`, `r`, `R`, `v`, `d`, `h`, `c`, `o`, and `z` key.
4. Specify persistent wide and compact master/detail interaction maps with terminal wireframes,
   focus ownership, one-level back/forward traversal, selection-driven detail updates, an explicit
   action menu, pane-owned forms/progress/results, and modal use limited to destructive or force
   confirmations. Define which summary, folder, agent, conversation, recovery, and technical
   evidence appears at each disclosure level without adding a message editor to Projects.
5. Walk the proposal through new creation, active work, unassigned and blocked projects, multiple
   folders, ownership conflict, agent handoff, pending-delivery recovery, close/reopen,
   archive/restore, stale or uncertain operations, and narrow terminals. Record invariants and a
   staged migration away from `UiProjectModal::Details`, including test seams and removal gates but
   no production implementation in this task.
6. Link the focused specification from `docs/rust/tui.md` and reconcile any nearby vocabulary that
   still describes project details as a dialog-owned ordinary interaction. Verify the document
   names every current datum and shortcut, uses the actual workflow semantics, and preserves Inbox
   as the only message-composition surface.
7. Run formatting and documentation checks plus the repository architecture/qualification
   validations, strict locked workspace Clippy, and the complete locked workspace test suite to
   prove the design-only change did not disturb the current baseline. Commit the review draft with
   a Conventional Commit, then stop for user review of the nouns, primary action, responsive
   layout, and contextual action matrix.
8. After explicit user approval, incorporate requested specification changes, add a separate
   front-of-queue implementation task with test-first model, responsive rendering, installed PTY,
   accessibility, documentation, and old-modal removal coverage, then archive this completed design
   task verbatim in `COMPLETED.md`. Do not start that implementation under the design-task approval.

Risks and open questions: `UiProject` currently carries technical history and pending-input data
that the details dialog does not render, so moving the same struct wholesale into a persistent pane
would preserve the coupling this design is meant to remove; `h` is both the current details-dialog
handoff shortcut and the global one-level back key; archive is presentation state at reduction but
the archive command first closes an open project through the release workflow; and a project may
have multiple Inbox conversations, so the primary action needs a deterministic, understandable
choice between continuing the most relevant conversation and opening the Inbox project set without
silently inventing identity or recency policy.

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
