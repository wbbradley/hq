# HQ

## Next Up

### Archive conversations and make Inbox agent-oriented

Replace message-by-message archiving with a conversation lifecycle, make `d` archive the selected
conversation, and make Inbox a stable roster of the people available to work.

- Define one shared typed conversation identity at the domain/application boundary for project
  threads, direct threads, and provider sessions. Add conversation-level archived state and remove
  the current `MessageArchived`/`MessageRestored`, mailbox `Archive`/`Restore`, and TUI `a`/`u`
  behavior wherever they exist solely to archive individual messages. Archiving must retain the
  complete immutable transcript and technical evidence as one read-only entry in Archived;
  subsequent facts racing with the operation remain part of that archived conversation rather than
  reopening it. Do not derive archive targets from the selected message, row text, current project
  assignment, provider display name, or page position.

- Implement a retry-safe stop-then-archive operation across the application, local API, store, and
  node executor. Resolve the exact registered agent, provider, provider session, project/thread, and
  runtime operation from typed conversation evidence. For a conversation owned by an agent, always
  request an orderly provider stop, including when no turn appears active; an already-idle or
  already-stopped runtime is successful and idempotent. Commit the conversation archive only after
  the runtime is definitely stopped. A rejected stop leaves the conversation active with an
  actionable retry, while an uncertain result retains its stable operation identity and reconciles
  without issuing a different archive or silently claiming success. Personal notes and future
  human-to-human conversations archive without attempting agent control. Reuse the existing
  managed-session and durable workflow boundaries where their identities and recovery contracts
  fit rather than sequencing unrelated fire-and-forget TUI effects.

- Change `d` in both Inbox-list focus and open-conversation focus to open a plain-language
  confirmation for the exact selected conversation, then run the stop/archive operation and refresh
  to the same stable agent row. `d` must be inert with contextual guidance when the selected row has
  no conversation yet or is diagnostic. Move direct-message creation exclusively under the `n`
  launcher's existing Direct Message choice, and update footer/help/modal text so `d` never means
  "message." Archived transcripts remain inspectable, but ordinary message-level archive/restore
  controls and commands disappear.

- Build Inbox rows from the complete active registered-agent catalog plus non-agent conversations
  that must remain distinct, instead of showing only conversations with open-message counts.
  Represent every active registered agent by stable `AgentId`, with typed optional current-
  conversation identity and typed optional project assignment; show the assigned project in
  ordinary row detail and retain exact agent/project/provider/session evidence in technical details.
  An agent with no conversation must still be selectable. Do not merge agents or conversations by
  name, mailbox label, title, or adjacency.

- Order agent rows by most recent canonical communication, using an explicit reducer/application-
  provided presentation rank or equivalent typed source-order evidence. Never treat `latest_fact`,
  authored timestamps, database arrival order, or the existing snapshot/vector position as
  recency. Agents with no communication follow communicated agents in deterministic normalized-
  name/`AgentId` order. Keep the roster scrollable and complete; a visual viewport may show roughly
  ten rows, but no hard ten-agent truncation may make a registered agent unreachable.

- Give each agent row one deterministic current conversation: the canonically most recent
  nonarchived conversation associated with that exact agent. Preserve older nonarchived
  conversations as typed alternatives rather than losing them; expose their count and a
  deterministic way to select one when more than one exists. After archiving the selected
  conversation, the same agent row remains and either advances to its next nonarchived conversation
  or becomes a no-conversation row. Starting later work creates a distinct conversation identity and
  never mutates or appends to the archived transcript.

- Make Enter follow the row's typed state. If the agent has a current conversation, open that exact
  conversation. If the agent is assigned to a project but has no current conversation, enter the
  shared new-work flow with both the agent and project already selected and continue to provider/
  session choice and first-message setup. If the agent is unassigned, enter that same flow seeded
  with the agent, then choose an existing project or create a new project before binding and starting
  the conversation. Conflicted, retired, unavailable, or multiply assigned agents must retain typed
  recovery views rather than being treated as ordinary start targets.

- Refactor the existing `n` project-work coordinator in `crates/hq-tui/src/model.rs` from its fixed
  project-first chain into one deterministic prerequisite workflow with retained stable selections
  for agent, project, provider/session, and initial message. Its next node is the first unsatisfied
  prerequisite, so it can be entered from `n`, from an assigned agent row, or from an unassigned
  agent row without duplicating picker, creation-child, Back, refresh, or reconciliation logic.
  Preserve the existing typed project/agent creation continuations, project-setup draft persistence,
  stale-effect rejection, and exact-ID reselection. Presentation remains ordinary intent language
  and must not expose DAG, assignment, provider-session, reducer, or workflow terminology.

- Start with reducer/application contract tests for whole-conversation archive state, late/racing
  entries, permanent archived history, and exact identity separation. Add durable-operation tests
  for active, idle, already-stopped, rejected, uncertain, retried, reconnected, and out-of-order
  stop/archive results. Add pure TUI and rendering tests for `d` from list and transcript, no
  message-level archive controls, Direct Message under `n`, every agent represented, canonical
  recency ordering, more than ten agents, zero/one/many conversations per agent, project labels,
  archived-row advancement, and every seeded new-work/Back edge. Extend node/local-API/store contract
  tests and the installed PTY journey to prove an active turn stops before its whole transcript moves
  to Archived and the agent can immediately start a distinct conversation.

Likely implementation areas are `crates/hq-domain/src/{semantic_fact.rs,fact_catalog.rs,ids.rs}`,
`crates/hq-reducer/src/conversation.rs`,
`crates/hq-application/src/{messaging.rs,mailbox.rs,snapshot.rs,ports.rs}`,
`crates/hq-local-api/src/{protocol/v1.rs,conversion.rs}`,
`crates/hq-store/src/{database.rs,database/conversation.rs,operational.rs}`,
`crates/hq-node/src/tui_client.rs`, `crates/hq-tui/src/{model.rs,render.rs}`, their corresponding
contract/model/render tests, and the conversation, Inbox, TUI, local-API, storage, behavior-ledger,
and acceptance documentation.

Dependencies are canonical conversation presentation ordering; exact agent/project/thread/provider-
session correlation; the managed-session stop capability; retry-safe operation receipts and
reconnect reconciliation; and the existing typed New-workflow child continuations.

Completion is observable when Inbox always provides a reachable, recency-ordered row for every
active registered agent; `d` definitely stops the exact agent runtime before archiving the selected
complete conversation; no individual-message archive behavior remains; Direct Message is available
through `n`; archived history stays intact; and Enter on an agent with no conversation reaches the
same deterministic project-binding and conversation-start flow used by global New.
