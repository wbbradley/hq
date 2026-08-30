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

### Move all message composition into the Inbox workspace

- Extend the durable draft vocabulary with a typed project target keyed by stable project ID and
  carry it exhaustively through `hq-tui`, the node TUI effect executor, local API v1, application
  planning, and store encoding. Resolve the project's current mailbox, account audience, and human
  authoring authority from transaction-consistent state; do not pretend a project message is direct
  messaging to whichever agent happens to be assigned.
- Separate the shared composer from modal presentation: replace `UiMailboxModal::Compose` with a
  modeless drafting pane owned by the Inbox/conversation workspace while retaining its durable
  behavior for replies, direct messages, notes, and project drafts. Recipient selection or a
  destructive confirmation may remain a bounded dialog, but message writing itself must not be.
  Preserve autosave, restart recovery, optimistic conflicts, bounded Unicode editing,
  save-on-Escape, and atomic draft consumption. A reply or follow-up authored from an open project
  conversation must retain that exact project-exchange key and return to the same Inbox row. A
  separately invoked `New conversation` action may create another exchange for the same project;
  the distinction must be explicit rather than inferred from provider session or current selection.
- In the guided `n New…` flow, submitting the first project message must continue to establish the
  exact activation thread and dispatch the retained input once, but completion must select and load
  the typed project conversation in `UiSection::Inbox`. It must not open a fresh
  `UiProjectModal::SendInput`. An ordinary subsequent project send must remain in the same Inbox
  conversation instead of mapping `InputSent` to `UiProjectCompletionContinuation::Details`.
  Remove `open_project_message_composer`, `ComposeInput`, every `UiProjectModal::SendInput` entry
  point, its form/rendering code, and `n send instructions` after all entry paths use the Inbox pane.
- While only one agent service is available, skip the `Start project work` confirmation dialog and
  proceed with that sole valid choice. Keep this conditional on the typed service catalog so the
  choice can return when it becomes meaningful.
- Keep the project-send command underneath for CLI and application compatibility, exact first-thread
  activation, automatic ordered pending-input dispatch, idempotency, and response-loss recovery.
  Keep failures beside the conversation/composer with the draft intact and a plain next action; a
  stale, archived, conflicted, or unassigned project must not silently retarget another mailbox,
  discard text, fall back to Project details, or expose routine dispatch as a normal user step.
- Cover durable project draft encoding/restart, autosave/submit races, duplicate completion,
  existing runnable projects, `conflict/project_activation_thread_missing`, and the complete
  post-bootstrap single-service path in store, local API, node mapping, pure-model, render, and
  installed PTY tests. Update `docs/rust/tui.md` and the acceptance walkthrough so the modeless
  Inbox workspace is the sole project-message experience and common writing surface for ordinary
  mailbox actions.

#### Implementation plan

1. Make a project exchange an authoritative draft/message target without introducing a second
   protocol generation. Modify `crates/hq-domain/src/semantic_fact.rs`,
   `crates/hq-application/src/messaging.rs`, `crates/hq-reducer/src/conversation.rs`,
   `crates/hq-reducer/src/project.rs`, `crates/hq-protocol/src/dto/{model,author,decode,semantic}.rs`,
   `crates/hq-store/src/database.rs`, and their focused tests so FCT-016 can either initiate an
   asynchronous thread or causally continue an exact existing project thread. A continuation will
   retain the original thread ID, project/account audience, project mailbox, and human sender; it
   will cite the root fact and fail closed for a missing, mismatched, non-project, or unusable root.
   Change the current v1/domain shapes in place: this pre-release work deliberately adds no legacy
   decoder, migration, schema version, local-API version, or compatibility fallback.
2. Extend `MailboxDraftTarget`, `MailboxCommandAction`, and the command planner in
   `crates/hq-application/src/mailbox.rs` with `Project { project_id, thread_id }`, where an absent
   thread explicitly means New conversation and a present thread means Continue conversation.
   Resolve the project projection, immutable mailbox, active account membership, local human
   mailbox fact, and exact root from the transaction snapshot. Add application tests first for a
   new project root, an exact continuation, and rejection of archived/conflicted/unassigned,
   mismatched, and stale targets; preserve atomic draft consumption on every rejection.
3. Carry that target exhaustively through local API v1 and persistence by modifying
   `crates/hq-local-api/src/protocol/v1.rs`, `crates/hq-local-api/src/conversion.rs`,
   `crates/hq-store/src/database.rs`, `crates/hq-store/tests/mailbox_drafts_contract.rs`,
   `crates/hq-store/tests/mailbox_command_contract.rs`, and focused local-API tests. Extend the
   existing v1 `mailbox_drafts` representation in place with project/thread columns and checks,
   cover Unicode/restart/conflict/round-trip behavior, and bind project/thread fields into command
   digests. Expose the root message/thread evidence already known by conversation summaries so a
   successful new send can be selected exactly after refresh instead of guessing by timing.
4. Replace modal composition state in `crates/hq-tui/src/model.rs` with a workspace-owned
   `UiMailboxDraftPane` and a typed Draft focus. Keep only recipient selection and destructive
   confirmation in `UiMailboxModal`; route editing, autosave timers, save-on-Escape, optimistic
   conflicts, submit-in-flight state, duplicate/stale completions, and failures through the pane.
   Entering any reply, direct message, note, or project target will switch to Inbox, retain/select
   the owning conversation when one exists, and open the common draft pane. Left/`h` will move
   Draft -> Conversation -> Inbox list -> top navigation, while the selected conversation remains
   visible.
5. Modify `crates/hq-tui/src/render.rs` and render tests to place the composer inside the Inbox
   master/detail workspace, using the remaining width and ordinary pane dividers rather than a
   centered rectangle. Show target and plain send/save/failure status beside the draft, keep the
   durable text visible through failures, and support both wide and compact layouts without hiding
   the selected conversation. Remove `UiFormKind::MailboxCompose`'s modal assumptions while
   preserving Unicode-safe cursor rendering and the existing highlighted field treatment.
6. Modify `crates/hq-node/src/tui_client.rs` and its executor/model tests to map typed project draft
   targets and mailbox actions to v1, return enough typed committed-message identity for exact
   post-refresh selection, and retain CLI/project-send as the lower-level root-send path. Extend
   snapshot row metadata with a typed conversation key rather than parsing row IDs or display text.
   Test restart recovery, save/submit races, response-loss duplicate completion, and a stale target
   that leaves the draft in place with an actionable failure.
7. Remove the parallel project-dialog writing path in `crates/hq-tui/src/model.rs` and
   `crates/hq-tui/src/render.rs`: delete `UiProjectModal::SendInput`, `UiProjectFormField::Content`,
   `UiFormKind::ProjectInput`, `UiProjectAction::SendInput`,
   `UiProjectCompletionContinuation::ComposeInput`, `open_project_message_composer`, the Details
   `n send instructions` shortcut, and all related rendering/input branches. Route the guided first
   instruction and ordinary runnable-project entry points into the typed Inbox project draft;
   after its first committed input, retain its exact thread through activation and then select/load
   that Inbox row. Never route success or `conflict/project_activation_thread_missing` to Project
   details or an empty replacement composer.
8. Simplify the guided service path in `crates/hq-tui/src/model.rs` and
   `crates/hq-tui/src/render.rs`: when the typed provider catalog contains exactly one available
   service, proceed directly through instruction/activation without `Start project work`; retain
   provider selection/review only when there is a real choice or a handoff/force decision. Cover a
   new project, an already-runnable project, no service, multiple services, and the single-service
   post-bootstrap flow in pure-model and render tests.
9. Update `crates/hq-node/tests/tui_effect_executor.rs`,
   `crates/hq-node/tests/tui_terminal_shell.rs`, `crates/hq-node/tests/unix_tui_terminal.rs`, and
   the installed PTY scenario fixtures to assert the complete user path: create from a folder,
   write the first message in Inbox, activate once, remain on the exact conversation, continue it
   without creating a third thread, explicitly start a second conversation, recover a preserved
   project draft after restart, and keep activation-thread failures next to the composer. Update
   `docs/rust/tui.md` and `docs/rust/acceptance-scenarios.md` so Inbox composition is the only
   ordinary writing surface and Project details no longer advertises message entry.
10. Run formatting and the repository architecture/qualification checks, strict locked workspace
    Clippy, the complete locked workspace test suite, and the installed PTY scenarios. Fix warnings
    and regressions before committing with a Conventional Commit.

Risks to check during implementation: continuing FCT-016 changes a canonical v1 wire shape and
therefore every exhaustive semantic-family adapter must change atomically; a newly committed root's
thread ID is fact-derived, so selection must use authoritative root-message evidence rather than a
locally predicted fact ID; and guided activation observes several asynchronous snapshots, so its
continuation must be identity-scoped and idempotent to avoid duplicate sends or stale navigation.

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
