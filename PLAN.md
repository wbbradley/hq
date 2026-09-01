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

### Keep Inbox details revision-coherent

Eliminate the transient `Loading messages…` detail pane and stale list/detail combinations by
treating the Inbox rows and the selected conversation's bounded first page as one revisioned
presentation. The daemon already owns the reduced canonical state; the TUI observation path should
deliver that usable state instead of discarding a subscribed snapshot, issuing another snapshot
request, and then loading the selected conversation separately.

- Stop reducing `ClientEvent::Snapshot` to revision-only `TuiClientObservation::Invalidated` in
  `crates/hq-node/src/tui_client.rs`. Carry a typed materialized observation into the reducer without
  giving the observation adapter mailbox mutation authority or sharing its transport with the
  command worker.
- Extend the unshipped local API v1 subscription contract in `crates/hq-local-api` so initial
  activation and later updates can include the authoritative snapshot and the bounded first page of
  one explicitly selected conversation at the same revision. Selection must use the stable typed
  `ConversationKeyDto`, not a presentation row ID, title, list position, or message body.
- Let the TUI update its selected-conversation interest over the existing bidirectional daemon
  connection. Coalesce rapid selection changes to the latest value, ignore stale projection
  responses, and preserve independent command progress and observation shutdown behavior.
- Build the combined projection in the daemon from one actor-owned reduced-state boundary. Do not
  duplicate canonical conversation reduction in the TUI, infer coherence from timestamps, or turn
  broad invalidation frames into unbounded message-body notifications. Keep ordinary CLI snapshot
  and explicit older-page queries available.
- Retain bounded conversation pages in `UiModel` by stable conversation identity. Apply a new list
  and its selected detail atomically; while a newer projection is in flight, keep displaying the
  last coherent matching view. If the selected Inbox row disappears after a reply, choose its
  deterministic successor and install that row's detail in the same transition rather than briefly
  showing an empty/loading detail pane.
- Reserve on-demand pagination for an explicit request for older history. Startup, reconnect,
  automatic refresh, send acknowledgement, row reordering, and ordinary Inbox navigation must not
  paint `Loading messages…` in the passive detail area.
- Add protocol/client/server tests for activation ordering, exact revision agreement, selection
  replacement, coalescing, reconnect, response loss, stale/out-of-order updates, bounds, and clean
  cancellation. Add model/executor/installed-PTY tests for rapid navigation, a selected row
  disappearing after send, simultaneous activity updates, and list/detail convergence without a
  loading flash or duplicate sent row.
- Update `docs/design.md`, `docs/protocol/local-api-v1.md`, and
  `docs/rust/{tui,node-lifecycle,acceptance-scenarios,behavior-ledger}.md` with the materialized
  observation boundary, revision coherence, bounded retention, and the distinction between current
  detail projection and explicit older-history pagination.

Acceptance criteria:

- The Inbox list and selected first-page detail shown in one frame come from one authoritative
  revision and stable conversation identity.
- The passive detail pane never flashes `Loading messages…` during startup convergence, navigation,
  reconnect, invalidation repair, or message submission; only explicit older-history pagination may
  show loading progress.
- A subscribed snapshot is not thrown away and fetched again, and one daemon change does not require
  an invalidation-to-snapshot-to-conversation request chain before the selected detail is current.
- Rapid selection, row disappearance, duplicated updates, response loss, and reconnect cannot show
  the wrong conversation, regress revision, duplicate a message, or grow retained pages without a
  bound.

### Acknowledge mailbox messages before project delivery

Give immediate, truthful send feedback and move project sequencing/runtime delivery out of the
mailbox command response path. A human message is first shown locally as pending; its durable commit
is acknowledged independently of later project reconciliation and provider submission.

This task depends on **Keep TUI invalidation observation independent from commands**.

- Enrich pending mailbox state in `crates/hq-tui/src/model.rs` so submitting a draft immediately
  places an optimistic local-human entry in the open conversation, anchored by the effect identity
  and labeled `Pending` before any daemon response. Preserve the exact draft body, target, and
  action until a definite outcome.
- Reconcile a committed receipt to the canonical message identity. Ordinary direct, reply, and
  self-note messages become `Sent`; project messages remain visibly queued until authoritative
  dispatch evidence appears. Use typed submission/delivery state rather than labels as evidence.
- On definite rejection, restore the exact editable draft with actionable failure context. On an
  uncertain response, retain the pending text and stable command identity so receipt reconciliation
  cannot duplicate the message.
- Refactor `NodeApplicationPorts::control_mailbox` in `crates/hq-node/src/components.rs` to return
  the durable store receipt without synchronously calling `reconcile_project_messages` or waiting
  for `HarnessSupervisor::deliver`.
- Give `ProjectNodeComponent` a coalesced asynchronous reconciliation trigger. A committed project
  message schedules work and returns; one serialized worker reconciles input and dispatches from
  durable state. Concurrent wakes coalesce, failures remain retryable, and startup, periodic repair,
  receipt replay, drain, and later relevant invalidations recover idempotently.
- Keep canonical message commit, project-input acceptance, dispatch records, and provider
  submission as distinct evidence. A background failure must not rewrite a committed mailbox
  receipt as failure or lose work after a crash between commit and wake.
- Add pure-model tests for immediate pending insertion, commit identity replacement,
  rejection/draft restoration, uncertainty, duplicate completions, focus/anchor preservation, and
  project queued-to-sent transitions. Add node/component tests with a blocked reconciler for early
  receipt, wake coalescing, unrunnable projects, delayed provider acceptance, failure/retry, and
  crash/startup recovery.
- Extend installed PTY coverage so Send paints the human message before a delayed fake Codex
  response, remains responsive during delivery, and converges without duplicate transcript rows.
- Update `docs/projects.md`, `docs/protocol/local-api-v1.md`, and
  `docs/rust/{tui,acceptance-scenarios,behavior-ledger}.md` with the submission, durable commit,
  queued delivery, and runtime-dispatch meanings.

Acceptance criteria:

- The TUI shows submitted text as pending in the same render cycle that emits the mailbox effect.
- A project mailbox response waits only for the durable mailbox mutation, not reconciliation,
  runtime start/resume, `turn/start`, or `turn/steer`.
- Background dispatch is serialized, idempotent, recoverable after restart, and cannot lose a
  committed input.
- Definite rejection preserves editable text; response loss and duplicate invalidations never
  author the message twice.

#### Implementation plan

1. Add pure-model tests in `crates/hq-tui/tests/model.rs` before changing submission state. Pin the
   same-transition optimistic local-human row for reply, direct, self-note, and project drafts; its
   effect-identity anchor, exact body/target retention, `Pending` delivery, conversation focus, and
   live-activity tail ordering; committed canonical identity replacement with `Sent` for ordinary
   messages and queued `Pending` for project messages; duplicate/stale completions; definite
   rejection restoring the exact editable draft and removing only its optimistic row; and uncertain
   completion retaining pending text without making a second authored row.
2. Replace the copy-only `PendingMailbox` marker in `crates/hq-tui/src/model.rs` with submission state
   that retains the draft, typed action, and optional optimistic entry identity. Insert the
   optimistic entry before emitting `SubmitMailboxCommand`. Reconcile that exact row in place on a
   committed receipt, preserve its logical anchor through reload, and let authoritative conversation
   pages replace it by canonical message identity. Keep archive/restore state-only commands and
   autosave/open-draft paths free of optimistic message rows.
3. Add a project-component wake capability and test-first worker contracts in
   `crates/hq-node/src/project_component.rs` and `crates/hq-node/tests/project_node_component.rs`.
   Move reconciliation over shared immutable worker/input capabilities into one named component-owned
   thread. A condition-variable latest-value signal must coalesce concurrent wakes, serialize each
   reconciliation pass, continue immediately when a bounded pass truncates, retry durable failures
   on the next wake or periodic repair, and perform a final pass before deterministic drain/join.
   Startup repair must precede admission, and forced stop must wake and join without inventing
   successful delivery evidence.
4. Refactor `NodeApplicationPorts` in `crates/hq-node/src/components.rs` so a committed project
   mailbox receipt schedules the reconciliation worker and returns immediately. Apply the same wake
   to later relevant canonical commits instead of running project reconciliation in the request
   dispatcher. Add component tests with a deliberately blocked project reconciler proving the
   mailbox mutation receipt returns first, duplicate wakes coalesce, unrelated/noncommitted mailbox
   outcomes do not schedule work, and wake failure after commit cannot rewrite the durable receipt.
5. Retain the existing canonical project-input and dispatch planners unchanged. Add worker tests for
   unrunnable inputs, delayed runtime/provider acceptance, rejected automatic commands, failure then
   wake retry, truncation, startup recovery, periodic repair, and shutdown with a wake in flight.
   The background owner must derive every retry from durable state; the in-memory wake carries no
   message body, project identity, or provider payload.
6. Extend `crates/hq-node/tests/tui_effect_executor.rs` and the installed PTY scenarios in
   `crates/hq-node/tests/unix_tui_terminal.rs` so sending paints the local-human row before a blocked
   command/provider path completes, terminal input and redraw remain responsive, the durable receipt
   changes ordinary delivery to sent while project delivery stays queued, and authoritative
   convergence produces no duplicate transcript row.
7. Update `docs/projects.md`, `docs/protocol/local-api-v1.md`, and
   `docs/rust/{tui,acceptance-scenarios,behavior-ledger}.md` with the distinct optimistic submission,
   durable mailbox commit, accepted project input, dispatch, and provider-delivery evidence. Run
   formatting, architecture and qualification validation, strict workspace Clippy, focused
   model/component/executor/PTY tests, and the complete locked all-target/all-feature workspace
   suite. Commit with a Conventional Commit, remove this task from `PLAN.md`, and append its
   implementation summary plus this complete original entry to `COMPLETED.md`.

Risks and open questions: an optimistic row has no canonical message identity until the receipt and
must never be mistaken for action authority; response loss must retain one stable in-flight command
without enabling a duplicate submission; a wake can be lost after commit or during process death so
startup/periodic repair must remain sufficient; the background worker shares command/input adapters
with foreground project operations and therefore requires explicit serialization/thread-safety
bounds; and shutdown must not deadlock when reconciliation is blocked in a bounded provider call.

### Let humans resolve provider requests

Expose provider-neutral questions, command/file approvals, permission requests, and MCP
elicitations through the local API and TUI. When no explicitly registered responder can receive
them, fail closed immediately instead of leaving an agent blocked invisibly.

This task depends on **Keep TUI invalidation observation independent from commands**.

- Keep `hq-harness` as the provider-neutral request/response owner, but add application-level
  passive interaction records and query/control ports instead of importing provider or supervisor
  types across layers. Carry stable agent, provider, session, operation, request kind, prompt,
  bounded choices, and request identity; reject secret-marked input.
- Extend the unshipped local API v1 in `crates/hq-local-api` with bounded pending-interaction
  queries, an exact-once answer/cancel command with stable command identity and digest, and
  responder registration tied to a local server session.
- Model registration like revision subscriptions: pending before its acknowledgement, active only
  after the acknowledgement frame is confirmed written, and cancelled on disconnect. Multiple TUI
  sessions may observe requests; the first valid terminal answer wins, an identical retry
  reconciles, and a changed answer under the same identity conflicts.
- In `HarnessNodeComponent` and `HarnessSupervisor`, retain requests only while an active responder
  exists. If a request arrives without one, or the last responder disconnects, send the adapter's
  fail-closed response for every outstanding request group: cancel questions/forms/URLs, decline
  command and file changes, and deny permission. A noninteractive CLI connection is not a
  responder.
- Publish an `operations` invalidation when a request appears, terminates, expires with its worker,
  or is failed closed. Never put prompt bodies in invalidation frames.
- Add a TUI interaction queue/modal with ordinary-language agent/project context, the exact prompt,
  humanized labels backed by untouched stable values, permitted free-text support, and explicit
  approve/deny/cancel controls. Give stale/already-answered requests a clear recovery path; preserve
  source ordering for grouped questions and terminate every member.
- Present a pending interaction as the conversation's current live status so “Approval needed” or
  “Alice needs an answer” supersedes generic working telemetry. Keep technical request identity and
  operation correlation in the inspector.
- Ensure stopping an agent, daemon shutdown, provider EOF, terminal turn evidence, or responder loss
  clears the UI request and fail-closes any provider request still owned by HQ.
- Add adapter/supervisor tests for every request kind, grouped questions, no-responder arrival,
  last-responder disconnect, duplicate/equal answer, changed-answer conflict, stale request, bounded
  queues, secret rejection, and shutdown. Add local API session tests for acknowledgement
  activation, cleanup, races, response loss, and reconnect. Add TUI model/render/PTY tests for every
  answer shape and a fake-Codex approval round trip.
- Update `docs/codex-adapter-v1.md`, `docs/design.md`, `docs/protocol/local-api-v1.md`, and
  `docs/rust/{tui,acceptance-scenarios,behavior-ledger}.md`.

Acceptance criteria:

- Every supported provider request reaches either one explicit terminal human response or an
  immediate fail-closed adapter response.
- An agent cannot wait indefinitely on a request that no active HQ client can display.
- Prompts and choices remain bounded and non-secret; invalidations remain body-free.
- Disconnect, duplicate answers, response loss, provider exit, and shutdown are exact-once and
  leave no stale TUI request.

### Keep only current activity at the conversation tail

Correct the centralized live-tail aggregation so completed command output and pre-message telemetry
cannot remain pinned as current work. Preserve pagination-safe store queries and typed activity.

This task depends on **Let humans resolve provider requests** so a pending approval can become the
authoritative live state.

- Extend `load_conversation_live_tail` in `crates/hq-store/src/database.rs`; do not reintroduce
  page-local inference. A `Progress` candidate is eligible only while its exact
  source/provider/session/operation is running and its exact item has no later typed
  `CompletedItem` evidence.
- Compare activity by typed correlation and source sequence, not prose, timestamps, or SQLite
  arrival order. Reverse ingestion and repair must retire output deltas when their command, file, or
  tool item becomes terminal.
- Treat a locally authored human message newer in canonical conversation order than selected
  progress as a freshness boundary. Until provider activity advances after that input, do not pin
  older progress beneath the message; fall back to typed running-turn status if the operation is
  still active.
- When a correlated provider interaction is pending, make it the sole live conversation status.
  Do not simultaneously show old command output, generic working telemetry, and the request.
- Preserve terminal `AgentTurn` replacement, concurrent-operation isolation, continuation-page
  behavior, repair/reopen determinism, the 200-progress retention contract, and logical
  anchor/follow-tail behavior.
- Remove redundant TUI reordering once the query supplies the authoritative live presentation, or
  narrow it so it cannot independently promote arbitrary `Running` progress.
- Add a regression for the reported sequence: failed-test output delta, typed command completion,
  provider approval request, then a newer “Are you still working?” human input. The old error stays
  in completed history/technical evidence, the human message keeps canonical placement, and the
  approval is the sole live tail.
- Add store/reducer/query tests for multiple items, item completion before/after progress arrival,
  sequential and concurrent operations, newer human input, later fresh progress, terminal turn,
  long paging, reopen, and repair. Add TUI mapper/model/render/PTY coverage for the pinned-error
  regression and stable selection.
- Update `docs/rust/{conversation-model,inbox-conversation-surface,acceptance-scenarios,behavior-ledger}.md`
  with item-terminal retirement, human-input freshness, and interaction precedence.

Acceptance criteria:

- Output from a completed command, file, or tool item is never selected as current live progress.
- Progress predating a newer human input is not moved beneath that input as if it were a response.
- A pending approval/question is the sole live tail; after it resolves, later provider activity or
  terminal lifecycle evidence determines the tail.
- Canonical facts and bounded technical evidence remain retained when ordinary presentation stops
  treating them as live.

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
