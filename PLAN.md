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

### Add typed conversation voices and activity presentation

Establish the typed data boundary required by the approved Inbox conversation design, then switch
ordinary entry rendering away from protocol taxonomy and IDs without yet taking on the responsive
viewport, theme-role, or inspector work.

- Preserve the reserved local-human mailbox and resolved conversation participant as typed display
  context from the authoritative snapshot through the node's conversation-page mapper.
- Classify every message author as `You`, the named/fallback participant, or unknown from exact
  mailbox evidence. Display labels must never become routing or action authority.
- Extend the closed activity kind with an agent-turn variant, retain activity kind in the reducer
  projection and store schema-v1 projection rows, and expose kind through local API v1 without a
  migration, compatibility branch, or version bump.
- Replace `UiConversationEntry`'s free-form kind/summary/content combination with typed message
  and activity presentation carrying author/body or kind/status/summary/exact detail. Preserve
  message targets and all technical evidence independently.
- Render the typed participant heading, optional project context, author/body blocks, and compact
  activity lines at column zero using existing theme roles as an intermediate presentation. Remove
  ordinary purpose/ID, `message · open`, and `update · information only` chrome now; leave width,
  measured scrolling, dedicated theme roles, full-row focus, and the inspector to the next task.
- Add domain/protocol/reducer/store/local-API/node/TUI tests first for exact author classification,
  unresolved fallbacks, every activity kind/status, agent-turn typing, schema-v1 persistence,
  technical-detail retention, action capability, and absence of obsolete ordinary labels.

#### Implementation plan

1. Add failing domain/protocol normalization tests in
   `crates/hq-codex/src/{normalize.rs,tests.rs}`,
   `crates/hq-protocol/{src/dto/{model,author,semantic}.rs,tests/semantic_conversion.rs}`, and
   relevant fact-catalog vectors proving Codex turn lifecycle records use a new closed
   `ActivityKind::AgentTurn` while status, progress, plan, diff, and completed-item semantics remain
   unchanged and exhaustive.
2. Extend `ActivityKind` in `crates/hq-domain/src/semantic_fact.rs`, its protocol author/decoder
   maps, normalization, digests, fixtures, and exhaustive tests. Keep canonical fact family/version
   1 and storage schema version 1; change current pre-release shapes in place without a legacy
   decoder or migration.
3. Add failing reducer and store contracts for retaining `ActivityView.kind` through reduction,
   repair, reopen, indexed conversation paging, and corruption checks. Modify
   `crates/hq-reducer/src/conversation.rs`,
   `crates/hq-store/src/database.rs`, and
   `crates/hq-store/src/database/conversation.rs` so every activity projection row stores its
   closed kind explicitly, including durable completed items whose projection key alone does not
   contain the kind. Expand the schema-v1 kind bound and exact dump/corruption coverage in place.
4. Add `local_human: MailboxAddress` to the application `ConversationSummary` in
   `crates/hq-application/src/snapshot.rs` and populate it from the already authoritative
   presentation policy in `crates/hq-store/src/database.rs`. Carry it as
   `MailboxAddressDto` on local API v1 conversation summaries, add a closed
   `ConversationActivityKindDto` to activity page entries, update conversion/validation, and write
   strict round-trip/incoherence tests in `crates/hq-local-api/{src/{conversion,protocol/v1}.rs,
   tests/protocol_v1.rs}`.
5. Replace the TUI entry's parallel `kind`, `content`, and free-form `summary` fields in
   `crates/hq-tui/src/model.rs` with a closed presentation enum: message author plus body, or typed
   activity kind/status plus bounded ordinary summary and exact detail. Add conversation title and
   optional project-context fields to `UiConversationPage`; retain stable fact identity,
   `UiMessageTarget`, exceptional message state, and namespaced technical sections as independent
   fields.
6. Retain each row's snapshot context inside `LocalTuiClient` in
   `crates/hq-node/src/tui_client.rs` beside its exact conversation key. When its page returns,
   classify sender mailboxes against exact local-human and participant evidence; use `You`, the
   sanitized resolved participant name, `Project agent`/`Other participant`, or `Unknown sender`
   without parsing row titles or purpose strings. Map activity kind/status to a generic typed
   summary while preserving the unmodified content as detail and all current technical evidence.
7. Write mapper/executor and pure-model tests in
   `crates/hq-node/tests/tui_effect_executor.rs` and `crates/hq-tui/tests/model.rs` for project,
   direct, personal, unresolved, and conflicting author contexts; every activity kind/status;
   activity non-actionability; message reply/archive capability; stale page/context behavior; and
   stable selection/reload. Update all fixtures exhaustively rather than adding compatibility
   constructors.
8. Adapt `crates/hq-tui/src/render.rs` and focused render tests to the new typed enum. Render the
   conversation's participant title and optional project line, then unindented explicit author/body
   blocks or one compact status-symbol/activity-summary line. Hide normal open state, purpose,
   presentation, shortened sender ID, and `information only`; show archived/rejected only when
   exceptional. Keep current layout, entry-capacity estimate, selection marker/style, technical
   expansion, and general theme roles temporarily so the next task owns their coordinated removal.
9. Update `docs/rust/tui.md`, `docs/rust/inbox-conversation-surface.md`, acceptance scenarios,
   behavior-ledger evidence, and any protocol/storage documentation affected by the in-place v1
   activity-kind and local-human-context additions. Mark only the typed-presentation migration stage
   complete; do not claim the responsive surface is finished.
10. Run formatting, architecture/qualification validation, strict locked workspace Clippy, the
    complete locked all-target/all-feature test suite, and installed conversation PTY coverage. Fix
    warnings and regressions, commit with a Conventional Commit, then archive this subtask verbatim
    while leaving the responsive rendering task at the front of `PLAN.md`.

Risks and open questions: adding a canonical activity enum variant touches exhaustive semantic fact
adapters and published fixtures; the activity projection currently discards kind and completed-item
keys cannot reconstruct it, so persistence must carry it explicitly; personal/project author
classification is only honest when the summary retains the exact local-human mailbox; and this
subtask intentionally leaves the old percentage split and fixed-height viewport in place for one
stack layer while removing their most confusing visible labels.

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
