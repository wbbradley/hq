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

### Deliver project follow-ups automatically without replaying agent output

Make every committed human message to a runnable project flow through the existing durable project
input and dispatch workflow, including messages sent from Inbox with `r`, while ensuring agent
status and final-output messages can never become project inputs.

- Write failing tests that reproduce the observed path: deliver an initial instruction during
  activation, then commit a second project message through `ControlMailbox`. Prove the follow-up is
  accepted exactly once, automatically enters the pending-dispatch workflow, produces one harness
  delivery and one `ProjectInputDispatched` fact, exposes agent activity, and reaches the open
  conversation without a restart or explicit project command.
- Define one project-owned post-commit operation that sequences eligible messages and schedules
  bounded dispatch. Keep mailbox composition decoupled from provider details and reuse the existing
  project saga, ordered pending-input drain, stable delivery identity, and harness ledger rather
  than adding another queue or direct provider-submission path. Preserve deterministic retries
  across uncertain acceptance, dispatch, and response loss.
- Centralize project-input eligibility and enforce it both when the home reconciler selects a
  candidate and when canonical acceptance is planned or reduced. Accept only the human-authored
  project message purposes defined by the project model; reject `ProjectOutput` status/final
  messages even when they carry the same project ID and mailbox recipient. Add reordered-fact and
  malformed-acceptance tests proving agent output cannot advance the input sequence or enter the
  pending queue.
- Preserve lifecycle and concurrency behavior: closed, unassigned, blocked, or otherwise
  non-runnable projects retain accepted input without treating submission as failed; runnable
  assignments dispatch in sequence; simultaneous submissions remain at-most-once at the provider
  boundary; and a busy or reconcilable workflow retains a durable automatic-dispatch trigger for
  bounded repair instead of stranding work.
- Add node-level coverage for mailbox commit → input reconciliation → dispatch, plus an installed
  TUI regression that sends a real follow-up with `r`, observes working activity, and receives the
  response in the already-open conversation. Cover startup/drain recovery and prove agent output is
  not re-ingested when its conversation facts arrive before or after input acceptance.
- Treat contaminated local stores as disposable pre-release data: add no migration or in-place
  canonical repair. After the fix is built, use `scripts/hq-bootstrap` to reset the current local
  installation and verify the fresh onboarding, initial delivery, and follow-up-delivery journey.
- Update the project model, application-service architecture, and acceptance-scenario documentation
  so automatic delivery is normative and manual dispatch is only typed stalled-delivery recovery.
  Keep protocol/schema shapes unchanged unless the implementation proves a change is necessary.

Implementation map:

- `crates/hq-domain/src/semantic_fact.rs`: expose the shared typed distinction between project
  input purposes and project output.
- `crates/hq-projects/src/input.rs` and `crates/hq-projects/src/lib.rs`: apply the eligibility rule
  during candidate selection and acceptance planning, and derive bounded, stable automatic
  dispatch requests from authoritative pending-input state.
- `crates/hq-reducer/src/conversation.rs`, `crates/hq-reducer/src/project.rs`, and
  `crates/hq-testkit/tests/project_reduction.rs`: enforce payload/purpose and referenced-input
  invariants under arbitrary fact arrival order.
- `crates/hq-node/src/project_component.rs`, `crates/hq-node/src/components.rs`,
  `crates/hq-node/src/foreground.rs`, and `crates/hq-node/src/graceful_runtime.rs`: compose input
  reconciliation with project-owned automatic dispatch at mailbox commit, project-command,
  startup, and drain boundaries.
- `crates/hq-node/tests/project_node_component.rs`, `crates/hq-node/tests/node_components.rs`,
  `crates/hq-node/tests/support/mod.rs`, and `crates/hq-node/tests/unix_tui_terminal.rs`: cover the
  component contract and the installed follow-up journey.
- `docs/projects.md`, `docs/rust/application-services.md`, `docs/rust/acceptance-scenarios.md`, and
  `docs/rust/tui.md`: document automatic delivery, typed eligibility, durable recovery, and the
  exceptional-only manual retry path.

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
