# Agent cancellation and ACP integration

Assessment against the repository and ACP v1 documentation on 2026-09-07.
This records the cancellation design and implemented control path. Installed
provider cancellation qualification remains
required before the parent implementation task is complete.

Implement cancellation through HQ's existing neutral harness contract now. Defer
an arbitrary ACP provider adapter until its submission-recovery guarantees can be
demonstrated. ACP is a useful integration boundary, but adopting its wire format
does not itself establish the durable acceptance evidence HQ requires.

## Existing path and gaps

| Layer | Current evidence | Required change |
| --- | --- | --- |
| Harness contract | `hq-harness/src/contract.rs` defines OperationCancellation, explicit outcomes, and a shared HarnessOperationControl handle independent of the mutable session. | Expose only declared capabilities backed by a live independent handle. |
| Registry | `hq-harness/src/registry.rs` rejects providers lacking StableSubmissionIdempotency or SubmissionLookup. | Preserve this admission requirement for future adapters. |
| Supervisor | `cancellable_worker` and `cancel_owned` use a separately locked handle registry, exact worker descriptors and live lease checks. | Route application cancellation through this path rather than the synchronous cancel convenience method. |
| Codex | `hq-codex/src/adapter.rs` maps an HQ operation to threadId/turnId and sends turn/interrupt. | Preserve exact targeting and distinguish accepted interruption from observed completion. |
| Application/node | ControlHarness exposes typed target queries, bounded cancellation admission and request observation. Canonical agent/session authority and exact generation/lease/operation are revalidated before independent dispatch. | Connect local API callers and prove installed end-to-end behavior. |
| Local API | Exact target/query/request-state DTOs route through independently owned per-session application work; responder cleanup also runs outside the coordinator. | Prove the installed cancellation action through the full TUI/provider path. |
| TUI | A dedicated bounded control worker owns a separate local client; Ctrl-G acts on the exact capability offered by the daemon. | Prove installed terminal persistence and continuation after cancellation. |

Paths in this table are relative to `crates/`.

The provider control path now bypasses the mutable session and supervisor workers
mutex. `HarnessNodeComponent::with_supervisor` clones its Arc before invoking the
callback. Codex's sole transport reader routes control responses by a separate
request-ID namespace to bounded waiters; ordinary RPC responses and notifications
still belong to the mutable session. Frame writes are serialized to prevent
interleaving. No second reader is introduced.

The node now admits cancellation into a bounded job registry outside ordinary
provider/workflow execution. Stable request identities retain outcomes; a changed
target under the same request identity is rejected. Retries of uncertain requests
cannot overlap their prior dispatch, and shutdown joins admitted controls before
starting a fresh generation. Query authorization resolves current project
assignments or selected direct agent sessions; human threads have no target.

The local API now exposes query, cancellation admission, and request observation
separately. The runtime owns application capabilities for independent request
workers. Per-session response/write order is preserved, and lifecycle requests
retain coordinator ownership for stop acknowledgement. Disconnected work and
responder cleanup remain bounded and joined before application shutdown. Gate
tests prove that a second client's control can release both a blocked request and
blocked cleanup on another connection.

The TUI now owns three joined workers: ordinary commands, subscribed observations,
and agent controls. A gate test proves the control worker can release an already
blocked ordinary command. Ctrl-G is offered only on an eligible conversation,
composer, or approval surface; it preserves the draft and reading position.
Uncertain retries retain the complete original target and wire request identity.
Request acknowledgement displays “Stopping…”; only a later exact canonical
terminal status establishes interruption or natural completion. Reads and receipts
are correlated to the selected conversation and effect identity, so navigation or
disconnect fences late responses. Human message threads have no control action.

Ordinary session methods still use the supervisor workers mutex; cancellation
uses cancel_owned rather than acquiring that lock.

Temporary submission backpressure retains the durable input in reconciliation
instead of rejecting it permanently. Since the submission attempt was already
recorded as uncertain, retries perform authoritative acceptance lookup first.
Project deliveries still require the project workflow's fresh eligibility check;
a generic worker wake does not drain them. Recovery tests cover both direct and
project input, exact retained identity, and acceptance after restart without
resubmitting accepted work.

Generation continues remotely after turn submission, so not every running turn
holds a Rust lock. This explains why the existing adapter can already interrupt
ordinary generation. Nevertheless, the required guarantee includes blocked
submission/RPC handling and sibling sessions; test those independently rather
than treating one successful interrupt as proof of concurrency.

The assessment found that HarnessCancellationOutcome::Cancelled meant request
acceptance and that Codex cleared active_turn immediately after the interrupt
response. The adapter now calls this outcome Requested and retains the active turn
until terminal provider evidence. Acknowledged duplicates retain their outcome;
uncertain requests can retry only the same exact live operation. Submissions receive
Backpressure while interruption is unresolved. The remaining
application integration must retain future inputs in the durable queue during that
interval rather than treating backpressure as permanent rejection. Finished/cancelled
UI state must come from authoritative operation evidence. Late output before
completion remains valid.

Provider history is used to recover an unambiguous active turn before live
lifecycle evidence arrives. After live evidence is available, history remains
submission-acceptance evidence and cannot replace the active turn. Known terminal
turn IDs reject late running notifications and fail closed on late questions.
A delayed start reply cannot revive its completed turn; lookup of an older input
cannot overwrite the operation binding established by a later accepted steer.
Deterministic adapter tests inject terminal/new-turn notifications before stale
history replies, repeat those reads, and check that cancellation still targets the
new operation. Multiple running history candidates do not select a control target
by their list positions.

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

The implementation uses the separate neutral control handle described above.
A test withholds an ordinary provider response until an independent interrupt
arrives, proving that control delivery does not require the mutable session owner
to return. Another test holds the supervisor worker registry during submission
and releases it only through exact-owner cancellation.

Control state arbitrates submission admission and cancellation: an old cancellation
cannot cross admission of a different input and accidentally stop its replacement.
Permission replies are claimed under the same state lock and written in that order.
Cancellation resolves pending permissions without requiring session polling, rejects
late approval answers, and suppresses queued questions already cancelled.

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
