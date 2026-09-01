# HQ

## Next Up

### Let humans resolve provider requests

Expose provider-neutral questions, command/file approvals, permission requests, and MCP
elicitations through the local API and TUI. If no explicitly registered responder can receive a
request, fail closed immediately instead of leaving an agent blocked invisibly.

- Keep `hq-harness` as the provider-neutral request/response owner. Add application-level passive
  interaction records and query/control ports; do not leak provider or supervisor types across
  layers. Preserve stable agent, provider, session, operation, request, and command identities.
- Extend local API v1 with bounded pending-interaction queries, exact-once answer/cancel commands,
  and responder registration tied to a server session. Registration becomes active only after its
  acknowledgement is written and ends on disconnect. Equal retries reconcile; changed reuse of an
  identity conflicts; the first terminal answer wins.
- Retain requests only while an active responder exists. No responder, last-responder disconnect,
  provider EOF, terminal turn evidence, agent stop, and daemon shutdown must fail-close every
  outstanding request group with the adapter's appropriate cancel, decline, or denial response.
- Publish body-free `operations` invalidations when a request appears or terminates. Keep prompts
  and choices bounded and reject secret-marked input.
- Add a TUI interaction queue/modal with ordinary-language agent/project context, exact prompts,
  humanized labels backed by stable values, permitted free text, and explicit approve, deny, and
  cancel actions. Preserve grouped-question order and show a pending interaction as the sole live
  conversation status.
- Cover every request/answer shape plus no-responder arrival, responder loss, grouped requests,
  duplicate/conflicting answers, response loss and reconnect, stale requests, bounded queues,
  secret rejection, provider exit, and shutdown. Include local API session races and fake-Codex TUI
  round trips.
- Update the Codex adapter, design, local API protocol, TUI, acceptance-scenario, and behavior-ledger
  documentation.

Done when every supported provider request receives exactly one explicit human response or an
immediate fail-closed adapter response, and no disconnect or terminal lifecycle path leaves either
the provider or TUI waiting on stale interaction state.

### Keep only current activity at the conversation tail

Correct the centralized live-tail aggregation so completed command output and pre-message telemetry
cannot remain pinned as current work. This depends on provider interactions becoming an explicit
live state in the preceding task.

- Extend `load_conversation_live_tail` in `crates/hq-store/src/database.rs`. A `Progress` candidate
  is live only while its exact source/provider/session/operation is running and its exact item has
  no later typed completion evidence. Preserve pagination-safe queries and the 200-progress bound.
- Compare activity by typed correlation and source sequence, not prose, timestamps, SQLite arrival
  order, or page-local inference. Reverse ingestion and repair must retire deltas for terminal
  command, file, and tool items.
- Treat newer locally authored human input as a freshness boundary until provider activity advances
  after it. If the operation remains active, fall back to typed running-turn status instead of
  pinning older progress beneath the message.
- Make a pending provider interaction the sole live conversation status. Preserve concurrent
  operation isolation, terminal-turn replacement, continuation paging, reopen/repair determinism,
  and logical follow-tail behavior.
- Remove or narrow redundant TUI reordering once the query owns the authoritative presentation.
- Regress the reported failed-test output, typed completion, approval request, then newer “Are you
  still working?” sequence. Cover completion ordering, multiple items/operations, later fresh
  progress, terminal turns, long paging, reopen/repair, and stable TUI selection.
- Update the conversation-model, inbox/conversation surface, acceptance-scenario, and
  behavior-ledger documentation.

Done when completed item output is never presented as current progress, activity older than a human
message is not moved below it, and a pending interaction exclusively owns the live tail without
discarding canonical history or technical evidence.
