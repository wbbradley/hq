# HQ

## Next Up

### Consume materialized Inbox details without loading flashes

Use coherent subscribed conversation views in the installed TUI so list and detail change together
and already-observed snapshots are never discarded and fetched again.

- Map subscribed materialized views in `LocalTuiObserver` and share their typed conversation
  directory with the independent command client without giving the observer mutation authority.
- Route selected-conversation interest over an interruptible latest-value observation control path;
  blocked commands must not delay selection or daemon updates, and shutdown must still join both
  workers deterministically.
- Retain a bounded set of first pages in `UiModel` by stable conversation identity. Apply list and
  selected detail atomically, keep the last coherent view while a newer one is pending, and ignore
  stale/out-of-order views.
- If a selected Inbox row disappears after send, choose its deterministic successor and install the
  matching detail together. Rapid navigation, reconnect, row reordering, activity updates, and send
  acknowledgement must never paint the wrong conversation or duplicate a sent message.
- Remove `Loading messages…` from passive first-page flows. Only explicit older-history pagination
  may show loading progress, and retained pages must have an explicit memory bound.
- Add model, mapper, executor, shell, and installed PTY tests for startup, rapid selection, blocked
  commands, row disappearance after send, reconnect, cache eviction, and authoritative convergence.
- Update `docs/rust/{tui,acceptance-scenarios,behavior-ledger}.md` with the materialized observation
  boundary, atomic presentation, bounded retention, and explicit older-history loading.

Acceptance criteria:

- The Inbox list and selected detail shown in one frame share a revision and stable conversation
  identity; a newer list is never paired with stale detail from another row.
- Startup, navigation, reconnect, automatic refresh, and message submission never flash
  `Loading messages…`; only explicit older-history pagination may show loading progress.
- Subscribed snapshots are consumed directly rather than converted to invalidations and fetched
  again, while commands and observations remain independently responsive.
- Rapid selection, row disappearance, duplicate updates, response loss, reconnect, and cache
  eviction cannot regress revision, show the wrong page, duplicate a message, or grow memory without
  a bound.

### Acknowledge mailbox messages before project delivery

Give immediate, truthful send feedback and move project sequencing/runtime delivery out of the
mailbox command response path. A human message is first shown locally as pending; its durable commit
is acknowledged independently of later project reconciliation and provider submission.

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

### Let humans resolve provider requests

Expose provider-neutral questions, command/file approvals, permission requests, and MCP
elicitations through the local API and TUI. When no explicitly registered responder can receive
them, fail closed immediately instead of leaving an agent blocked invisibly.

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
