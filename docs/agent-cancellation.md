# Agent cancellation and ACP integration

Assessment against the repository and ACP v1 documentation on 2026-09-07.
This is the design for the queued cancellation implementation, not a claim that
the conversation UI already exposes cancellation.

Implement cancellation through HQ's existing neutral harness contract now. Defer
an arbitrary ACP provider adapter until its submission-recovery guarantees can be
demonstrated. ACP is a useful integration boundary, but adopting its wire format
does not itself establish the durable acceptance evidence HQ requires.

## Existing path and gaps

| Layer | Current evidence | Required change |
| --- | --- | --- |
| Harness contract | `hq-harness/src/contract.rs` defines OperationCancellation, cancel_operation, and explicit outcomes. | Keep recipient capabilities and operation-targeted control independent of Codex. |
| Registry | `hq-harness/src/registry.rs` rejects providers lacking StableSubmissionIdempotency or SubmissionLookup. | Preserve this admission requirement for future adapters. |
| Supervisor | `HarnessSupervisor::cancel` renews ownership and invokes the live session while holding the workers map mutex. | Validate capability and exact live target; avoid shared locks spanning blocking provider work. |
| Codex | `hq-codex/src/adapter.rs` maps an HQ operation to threadId/turnId and sends turn/interrupt. | Preserve exact targeting and distinguish accepted interruption from observed completion. |
| Application/node | Project runtime observation already has assignment, session, generation, owner and running-operation evidence. | Add a typed authorized control port, separate from project workflow progression and message submission. |
| Local API | `hq-local-api/src/server.rs` handles requests synchronously; `hq-node/src/session_registry.rs` calls receive during dispatch. | Keep control dispatch responsive even when another command is waiting. A separate socket alone does not prove this. |
| TUI | `hq-node/src/tui_client.rs` processes WorkerCommand values in a receive loop. | Do not enqueue interrupt behind a blocked operation on the same client worker. |

Paths in this table are relative to `crates/`.

There are multiple serialization boundaries. `HarnessNodeComponent::with_supervisor`
holds its supervisor mutex during the callback. The supervisor then holds its
workers mutex while calling session methods. Codex's synchronous `rpc` waits for
a matching response while dispatching inbound events; it is not merely a write.
Moving the API operation to a thread without changing these lock lifetimes would
leave cancellation blocked and could stall sibling conversations.

Generation continues remotely after turn submission, so not every running turn
holds a Rust lock. This explains why the existing adapter can already interrupt
ordinary generation. Nevertheless, the required guarantee includes blocked
submission/RPC handling and sibling sessions; test those independently rather
than treating one successful interrupt as proof of concurrency.

The assessment found that HarnessCancellationOutcome::Cancelled meant request
acceptance and that Codex cleared active_turn immediately after the interrupt
response. The adapter now calls this outcome Requested and retains the active turn
until terminal provider evidence. Duplicate requests retain their original outcome,
and submissions receive Backpressure while interruption is unresolved. The remaining
application integration must retain future inputs in the durable queue during that
interval rather than treating backpressure as permanent rejection. Finished/cancelled
UI state must come from authoritative operation evidence. Late output before
completion remains valid.

## Control contract

Expose an optional cancellation target from authoritative recipient/runtime data.
A human conversation has no such target. An agent label or an enabled text editor
does not imply any agent capability.

The target needs actor/account and home scope, project/assignment where applicable,
agent, provider/session, runtime generation, lease owner and exact operation
identity. At execution, independently authorize the actor and compare canonical
assignment and live ownership/operation evidence. An old target must never resolve
to whatever turn happens to be current. A provider lacking OperationCancellation
must be rejected in the daemon even if a client fabricates the request.

Use a bounded control command and explicit outcomes: requested, already finished,
rejected and uncertain. Keep request identity stable across response loss, and
do not retry against a newer target. The UI should present Stop current work only
when supported, report Stopping after acknowledgement, and continue observing until
terminal state. It must preserve the draft and conversation reading position.
The keyboard action must work while composing without stealing ordinary text.

Provider I/O should have independently addressable control delivery with bounded
waiting, and no global supervisor/map guard held across RPC. Per-worker ownership
alone is insufficient if that same worker is blocked awaiting submission. Choose
a session actor with multiplexed outstanding requests or a separate neutral control
handle; the chosen design must maintain one reader/demultiplexer and exact response
correlation. Never create competing readers on the provider stream.

Cancellation of a turn is separate from declining an approval. Resolve pending
interactive requests belonging to that exact operation and ensure stale UI answers
cannot reactivate it. A naturally completed turn racing cancellation is an ordinary
terminal outcome. If delivery is uncertain, observe instead of claiming success or
resubmitting accepted input. Do not cancel queued future messages implicitly.

## ACP fit

ACP initialization negotiates protocol version, agent/client capabilities and
authentication. Cancellation is a baseline agent session method, while richer
prompt formats and lifecycle methods depend on advertised support. An ACP adapter
could advertise HQ OperationCancellation after successful initialization, but
must expose only capabilities it can actually honor. These are runtime contracts,
not decisions based on agent display names.
[Initialization](https://agentclientprotocol.com/protocol/v1/initialization)

ACP session/cancel is a notification scoped to a session, without an HQ-style
operation argument. Completion is reported through the original session/prompt
response with a cancelled stop reason; updates may arrive before that response.
Pending permission requests must receive the cancelled outcome. An adapter therefore
needs an exclusive mapping between an HQ operation and that session's active prompt,
and must prevent a delayed cancellation from reaching a subsequent prompt.
Transport-write success alone is not terminal cancellation.
[Prompt lifecycle and cancellation](https://agentclientprotocol.com/protocol/v1/prompt-turn)

Session loading can replay history; optional session resume reconnects without
replay. Both are documented in the current v1 lifecycle reference and must be
capability-gated. Neither mechanism by itself proves whether a particular HQ
submission was accepted before a disconnect. Optional message identifiers during
replay are not a substitute for a durable submission identity and digest.
[Session setup](https://agentclientprotocol.com/protocol/v1/session-setup)

The v1 PromptRequest schema contains sessionId, prompt and optional metadata; it
does not specify HQ's submission-id/digest idempotency or an acceptance lookup.
Metadata alone carries no agreed semantics. Therefore, a generic conforming agent
cannot be assumed to satisfy HQ's registry gate. An adapter must obtain a documented,
tested provider extension or equivalent authoritative evidence before registration.
This is an inference from the schema and HQ's current admission rule, not a claim
that no individual ACP agent can provide stronger guarantees.
[Protocol schema](https://agentclientprotocol.com/protocol/v1/schema)

ACP publishes an official Rust SDK for both sides of the protocol. TypeScript is
not required. Evaluate a pinned Rust SDK release and its executor/transport needs
when building a concrete adapter; do not introduce a dependency merely to implement
the existing Codex cancellation path.
[Rust SDK](https://agentclientprotocol.com/libraries/rust)

Treat the cited v1 contract as the baseline for this assessment, not a promise that
every older SDK exposes every currently documented method. Any preview or extension
used later needs explicit negotiation, a pinned schema/version, and separate tests.
HQ has no compatibility requirement with its own older builds.

## Bounded future ACP adapter

A future `hq-acp` crate would implement HarnessFactory/HarnessSession, own the
protocol transport and initialization, and translate typed session, tool,
permission and completion events. Keep ACP types inside that crate; application,
API and TUI consume HQ capabilities and identities.

Start with one concrete agent whose acceptance/recovery extension is documented.
Map HQ operation and submission IDs to the provider's durable evidence, including
reconnect behavior; preserve provider event identities and source sequence rather
than inventing replay identities from text or arrival order. Handle the original
prompt response separately from the cancel notification. Retain terminal state
before admitting another prompt to a session that has a pending cancellation.

This is a recommendation to defer arbitrary ACP support, not to weaken HQ's harness
contract or make cancellation depend on an ACP implementation.

## Evidence required for implementation completion

Use a deterministic provider fixture that gates generation/tool execution and
another that gates an RPC response. Issue cancellation through the real local
API/TUI control path and prove the provider receives it before either gate is
released. Check sibling status and control responsiveness during those waits.

Assert exact target and authorization rejection, unsupported and human recipients,
natural completion races, duplicate/stale requests, uncertain acknowledgements,
pending approvals, late events, authoritative terminal persistence, and successful
conversation continuation. Verify that stopping one operation neither resubmits
accepted work nor stops the next operation. A mock that returns Cancelled immediately
does not demonstrate these properties.
