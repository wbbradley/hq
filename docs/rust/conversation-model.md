# Conversation and activity reduction

Status: Active implementation contract

`hq_reducer::ConversationReducer` is the pure complete-batch policy for `FCT-015` through
`FCT-022`. It takes only a fact set plus explicit `AuthorityPolicy`, delegates historical signer,
scope, capability, and account checks to the authority stage, and returns normalized decisions,
frontiers, exact projection support, conflicts, and one presentation order. It does not read a
clock, receipt, relay, database, runtime, or UI value.

## Conversation identities and thread state

A question or asynchronous root derives its `ThreadId` from the root `FactId`. Every answer and
cancellation cites the question root. Answers reverse the direct sender/recipient route (or retain
the same account audience), preserve compatible typed purpose and correlation, and remain an
independent grow-only set. Cancellations remain a separate grow-only set. `ThreadView` exposes all
answers, all cancellations, canonical ready-answer order, and every answer/cancellation pair as
`Before`, `After`, or `Concurrent` using usable reachability.

`MessageId` is a stable public handle, not a winner key. Unequal canonical facts reusing one handle
are explicitly conflicted and cannot create a message, thread action, archive target, delivery
claim, or final-answer selection. Human text is retained only as bounded display content; strings
that resemble authority, correlation, technical labels, or final-answer markers have no semantic
effect.

An addressed answer whose required history is absent remains `unresolved`. The separate
`incomplete_addressed_observations` query exposes its typed address/content and exact missing or
unusable dependencies. That inert observation does not enter projections or presentation and
cannot support an answer, action group, final answer, archive, delivery claim, or project input.

## Message state and delivery evidence

Archive and restore form a remove-wins register over usable causal maxima:

- a maximal archive closes the message;
- a restore opens it only after it descends from every archive maximum;
- a later archive closes it again; and
- any usable rejection is absorbing, while a causally later restore is invalid.

Canonical message and state facts remain auditable. The active `MessageView` may retract and always
reports its exact state frontier and transitive support.

Peer receipt is semantic causal evidence, not transport status. A message records peer receipt only
when a usable child authored by another installation cites it. Relay acceptance, wrapper arrival,
receiver clocks, and local row order cannot create that evidence. One account-addressed canonical
fact has the same decision, message meaning, and presentation identity under every device-local
policy; encrypted fanout wrappers are outside reduction.

## Typed action groups and presentation

Messages with one `OperationCorrelation` form an action group. Every entry remains available, and
the group's selected final answer is the canonically last entry whose typed `PresentationKind` is
`FinalAnswer`. No body parsing participates.

Projected messages and reducer-retained activity feed the sole `canonical_presentation_order`
Kahn traversal. Parents are emitted before selected children even when authored clocks move
backwards. Among ready entries, the typed key uses authored time, signed occurrence time, family,
source, provider/session/operation/item, runtime, positive sequence, public identity, and fact ID.
Messages precede activity only on an exact ready-key time tie. These fields order presentation; they
never grant authority or resolve domain conflicts.

The authoritative client snapshot exposes conversation discovery summaries with the latest
presented fact plus open, archived, and reserved-local-human-authored message counts. These counts
are derived from the same projected message set and explicit local authority policy; they are
filter metadata, not canonical facts or ordering inputs. Clients load history only through bounded
opaque-cursor pages and must preserve the returned union order unchanged.

Conversation identity is a closed typed union. Ordinary uncorrelated messages use an exact causal
thread and counterparty; direct runtime output uses the exact counterparty/provider/session pair.
Project-addressed messages instead use `(project_id, thread_id)`. The initiating input's fact-derived
thread is retained by its dispatch and copied into typed project output and activity attribution, so
provider-session changes or assignment handoff cannot split one exchange. A separately initiated
message for the same project has a different thread and therefore remains a different conversation.
No content, current assignment, display name, or row adjacency participates in grouping.

## Non-actionable activity

`HarnessActivityRecorded` carries a full source mailbox, provider/session/operation and optional
item correlation, activity kind and status, logical key, runtime lifetime, positive source
sequence, signed occurrence time, bounded content, and explicit truncation. The source must be a
projected agent mailbox on the author's installation. Activity never creates or changes a message,
thread, inbox/action unit, unread state, reply/archive/draft target, delivery claim, or final answer.

Snapshot and progress winners are scoped by the exact source mailbox, provider, session, operation,
kind, item, logical key, and runtime. A higher semantic sequence wins even when concurrent; equal
sequence with unequal content conflicts instead of choosing by timestamp or fact ID. Concurrent
runtime lifetimes for one logical key are reported as a normalized conflict. Completed items remain
individual durable history rather than coalescing.

Canonical activity facts are permanent. The disposable view retains snapshot winners, every
completed record, and the canonically newest 200 progress winners per source/provider session.
Complete batch, reversed arrival, late-parent replay, and repair rebuild therefore select the same
facts and presentation order.

## Executable evidence

`crates/hq-testkit/tests/conversation_reduction.rs` maps and executes `CONV-001` through
`CONV-017` and `ACT-001` through `ACT-009`, including the `REG-002` comparator regression shape.
It covers exhaustive permutations of small answer/cancellation and mixed message/activity graphs,
missing history, public-ID collisions, all state transitions, typed prose inertness, peer receipt,
account fanout, final-answer grouping, typed project-exchange grouping, delayed occurrence,
runtime/provider/source namespaces, equal-sequence conflicts, completed history, and a 205-record
retention rebuild.
