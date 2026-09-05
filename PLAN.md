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

#### Implementation plan

Work through these dependency-ordered capability units, committing each coherent boundary before
continuing while keeping this complete task in `Next Up` until the end state is verified.

##### Persist whole-conversation state

- Move the reducer-owned `ConversationKey` shape to an application-independent
  `hq_domain::ConversationId` value and re-export it as needed so facts, reducer projections,
  application requests, protocol DTOs, storage keys, and the TUI all use the same typed identity.
- Replace the message-state fact families and protocol bodies with one permanent
  `ConversationArchived` fact carrying that identity. Update fact catalog/codec golden tests and
  delete message archive/restore planning and projection behavior while retaining rejection and
  question cancellation as their distinct semantics.
- Extend conversation reduction with an absorbing archived projection per exact conversation.
  Conversation summaries/pages gain a typed archived flag and a canonical presentation rank derived
  from the reducer's global presentation traversal. Late or concurrent entries for the same identity
  remain in its transcript and cannot reopen it.
- Primary files: `crates/hq-domain/src/{ids.rs,lib.rs,semantic_fact.rs,fact_catalog.rs}`,
  `crates/hq-protocol/src/dto/{model.rs,decode.rs,author.rs,semantic.rs}`,
  `crates/hq-reducer/src/{conversation.rs,lib.rs}`,
  `crates/hq-application/src/{messaging.rs,mailbox.rs,snapshot.rs,mutation.rs,lib.rs}`,
  `crates/hq-store/src/{snapshot.rs,database.rs,database/conversation.rs,database/repair.rs}`,
  `crates/hq-local-api/src/{protocol/v1.rs,conversion.rs}`, and their fact-codec, reduction,
  snapshot, incremental-query, and protocol contract tests.

##### Coordinate stop and archive durably

- Add a retry-safe conversation-archive command and durable record keyed by stable command and
  operation identities. Resolve the selected `ConversationId` against one authoritative snapshot,
  retaining exact agent, project, provider, session, and project-head evidence rather than labels.
- For project work, reuse the project close workflow to quiesce the runtime and release the
  assignment, then commit `ConversationArchived` without archiving the reusable project. For direct
  provider-session work, use the managed-session stop port before the same archive mutation; personal
  and non-agent conversations skip runtime control. Treat idle/already-stopped as accepted, retain
  rejected conversations as active, and checkpoint uncertain effects for exact reconciliation.
- Extend daemon recovery so nonterminal archive records resume after reconnect/restart and prevent
  a second command from racing the same conversation. Surface typed accepted/rejected/uncertain
  results through the application service and local API.
- Primary files: new capability-named workflow/store modules under `crates/hq-application`,
  `crates/hq-store`, and `crates/hq-node`; `crates/hq-projects/src/{lib.rs,workflow.rs}` only where a
  reusable close boundary is required; `crates/hq-local-api/src/{protocol/v1.rs,conversion.rs,
  client.rs,server.rs}`; node component composition; database schema/actor/gateway code; and focused
  active, idle, already-stopped, rejected, uncertain, retry, reconnect, and ordering tests.

##### Project the agent-oriented Inbox

- Extend the authoritative snapshot with typed agent-to-conversation candidates and current
  selection derived by exact `AgentId`, archived state, and canonical presentation rank. Preserve
  every older nonarchived candidate and deterministic no-communication ordering.
- Replace message-count-gated Inbox construction in `crates/hq-node/src/tui_client.rs` with one row
  for every active registered agent plus separate personal/future human conversations and diagnostic
  rows. Each agent row carries its exact agent identity, optional current conversation, optional
  assignment/project, alternatives, and technical evidence; the list is complete beyond ten rows.
- Keep selection stable across refresh and archive: advance the same agent row to its next active
  conversation or its no-conversation state. Add application/store/local-API projection tests and
  TUI-client tests for zero/one/many conversations, duplicate names, project labels, canonical
  recency, archive advancement, and more than ten agents.
- Primary files: `crates/hq-application/src/snapshot.rs`, `crates/hq-store/src/database.rs`,
  `crates/hq-local-api/src/{protocol/v1.rs,conversion.rs}`, `crates/hq-node/src/tui_client.rs`,
  `crates/hq-tui/src/model.rs`, and their snapshot/protocol/presentation tests.

##### Share the new-work prerequisites

- Replace the fixed project-first `UiNewWorkflow`/`UiNewModal` transition chain in
  `crates/hq-tui/src/model.rs` with a typed prerequisite state retaining optional exact agent,
  project, provider/session, and initial-message setup. A single deterministic transition function
  chooses the first unmet prerequisite and owns forward, Back, child project/agent creation,
  refresh, stale-effect, and recovery behavior.
- Seed that coordinator from global `n`, an assigned agent without a conversation, or an unassigned
  agent. Enter opens an exact current conversation when present; otherwise it chooses/creates the
  missing project and continues through the shared setup. Preserve explicit recovery for conflicted,
  retired, unavailable, and multiply assigned agents.
- Update `crates/hq-tui/src/render.rs` with plain-language prompts and add pure model/render tests for
  each seed and every forward/Back edge, including authoritative reorder and child-flow return.

##### Complete the archive interaction

- Add a conversation-level confirmation and progress/result state in `crates/hq-tui/src/model.rs`.
  `d` targets the selected row's exact current conversation from either list or transcript focus,
  submits one stable archive operation, and refreshes back to the same agent row. Rows without an
  archiveable conversation provide contextual help.
- Remove TUI/local-API/CLI message-level archive and restore actions and tests. Keep Direct Message
  only in the `n` launcher, and update `crates/hq-tui/src/render.rs`, contextual help, command
  grammar, and user-facing documentation so the shortcuts and outcomes are unambiguous.
- Extend installed PTY/acceptance coverage to demonstrate that an active runtime stops before the
  whole transcript appears read-only in Archived and that the same agent can immediately begin a
  new, distinct conversation bound to an existing or newly created project.

#### Risks resolved by the design

- Project closure and conversation archival are separate facts: archiving work releases the agent
  but leaves the project reusable instead of hiding the project as a side effect.
- Cross-conversation recency is a reducer-issued rank from canonical presentation order, never a
  comparison of random `FactId` bytes, timestamps, storage arrival, or UI vector position.
- Conversation archival is absorbing; there is deliberately no restore action. A later exchange
  with the same agent must receive a new conversation identity.

Completion is observable when Inbox always provides a reachable, recency-ordered row for every
active registered agent; `d` definitely stops the exact agent runtime before archiving the selected
complete conversation; no individual-message archive behavior remains; Direct Message is available
through `n`; archived history stays intact; and Enter on an agent with no conversation reaches the
same deterministic project-binding and conversation-start flow used by global New.

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
