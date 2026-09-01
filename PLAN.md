# HQ

## Next Up

### Let humans resolve provider requests

Expose provider-neutral questions, command/file approvals, permission requests, and MCP
elicitations through the local API and TUI. If no explicitly registered responder can receive a
request, fail closed immediately instead of leaving an agent blocked invisibly.

- Add bounded pending-interaction queries and exact-once answer/cancel commands to local API v1.
  Responder registration is session-scoped, activates after its acknowledgement is written, and
  reconciles identical command retries while rejecting conflicting reuse.
- Fail-close retained requests when no responder remains or their provider operation terminates,
  including provider EOF, terminal turn evidence, agent stop, and daemon shutdown.
- Show the interaction queue in the TUI with agent/project context, every supported answer shape,
  explicit cancellation, and grouped-question ordering. A pending interaction is the sole live
  status for its operation.
- Regress request/answer shapes, responder lifecycle races, duplicate/conflicting answers, response
  loss and reconnect, stale requests, provider termination, and the fake-Codex approval round trip.

Done when every supported provider request receives exactly one explicit human response or an
immediate fail-closed adapter response, and no disconnect or terminal lifecycle path leaves either
the provider or TUI waiting on stale interaction state.

### Keep only current activity at the conversation tail

Correct the centralized live-tail aggregation so completed command output and pre-message telemetry
cannot remain pinned as current work. This depends on provider interactions becoming an explicit
live state in the preceding task.

- Retire `Progress` from `load_conversation_live_tail` when its exact item has later completion
  evidence, while preserving concurrent-operation isolation and pagination bounds.
- Treat newer local-human input as a freshness boundary until later provider activity; use the
  running turn rather than older progress while the operation remains active.
- Make a pending provider interaction supersede other live status for its exact operation.
- Regress the reported failed-test output, typed completion, approval request, then newer “Are you
  still working?” sequence, including reopen/repair and stable TUI selection.

Done when completed item output is never presented as current progress, activity older than a human
message is not moved below it, and a pending interaction exclusively owns the live tail without
discarding canonical history or technical evidence.
