# Provider-neutral harness supervisor v1

Status: Normative Rust-era operational contract

This document defines the consumer-owned supervisor above the provider-neutral harness contract.
The supervisor coordinates external effects for one installation; its leases and ledgers are
operational state, not canonical facts and not authority.

## Worker ownership and readiness

There is at most one mutable worker owner for each named agent. A worker owns one provider session,
one fixed-capacity event buffer, one source-staging slot, one equally bounded interactive-request
queue, and one opaque 32-byte owner token. Before opening a provider session it MUST atomically
claim the agent's durable lease. A live exact owner may renew its lease. A different token may take
over only after the injected absolute deadline has expired.

Ready state commits only after `Start` or exact `Resume` acknowledges a provider session identity.
Every delivery transition and persistence checkpoint MUST cite the current exact token. Release
deletes only a matching token, so a stale daemon cannot mutate or release its successor. Failure to
open readiness releases the claim. Competing workers for different agents remain independent.

Managed start, exact-resume, and stop requests additionally use a durable operation ledger keyed by
the caller's stable operation ID. The immutable record binds the exact request digest, agent,
provider, action, and requested resume identity. `Prepared` advances to `Uncertain` before provider
I/O; `Ready`, `Stopped`, and `Rejected` are absorbing. Equal replay returns the retained result and
changed reuse fails closed. After restart, an uncertain start/resume becomes ready only when the
same live worker exposes the matching provider/session; an uncertain stop becomes stopped only when
no worker remains or the matching worker is authoritatively stopped. Otherwise uncertainty remains
explicit. No migration or compatibility representation is involved.

## Durable delivery and recovery

Before provider I/O, a delivery durably records its agent, provider, session, stable submission ID,
complete digest, operation, bounded neutral body, and queue time. Environment values, provider wire
payloads, diagnostics, and credentials MUST NOT enter the ledger. Equal immutable identity is an
idempotent replay even after state advances; unequal reuse fails closed.

Delivery state is monotonic:

- `Pending` means no provider call has begun.
- `Uncertain` is checkpointed before the first call and means acceptance needs reconciliation.
- `Accepted` is an absorbing authoritative acceptance.
- `Rejected` is an absorbing authoritative rejection and is never retried.

A pending delivery advances to uncertain before submission. After response loss or daemon restart,
an uncertain delivery uses exact submission lookup before retry. Authoritative acceptance advances
to accepted without resubmission. Definite absence permits only the exact recorded retry. Collision
or indeterminate lookup fails closed. Starting an exact resumed worker immediately wakes its durable
pending and uncertain records; ordinary repair wakes may safely repeat because terminal records are
excluded and all transitions are idempotent.

Each state read is explicitly bounded. Direct exact-identity reads support client replay without
scanning or regressing a terminal record. A caller that owns more runnable rows than one bounded
repair pass MUST schedule subsequent coalesced wakes until no eligible row remains.

## Event acceptance and persistence

Each worker owns a fixed-capacity FIFO. Durable output and activity items are never evicted. A new
item at capacity returns backpressure with ownership of the rejected value. Replaceable activity
snapshots may coalesce only with the same operation/logical key already in the buffer: the older
snapshot is removed and the replacement is appended at the tail, preserving the order of all
intervening work. A new snapshot key at capacity also backpressures.

The node owns one joined event task for the complete supervisor, not one detached task per
provider. Every adapter shares the task's retained coalescing notifier. After a wake, every bounded
pass drains each live session nonblockingly in stable agent order; exhausting the fairness budget
self-signals an immediate continuation. The normalized source event is admitted before that session
is drained again. If a distinct durable
value arrives at capacity, the worker retains it in its single staging slot and stops polling that
source until admission succeeds. Thus pressure is bounded to the configured FIFO plus one already
polled value, durable values cannot disappear, and an exact snapshot may still replace its pending
pre-persistence predecessor. Output uses its provider-normalized output identity; activity derives
its checkpoint identity from operation, item, kind, logical key, runtime, and semantic sequence.
The complete normalized value determines the checkpoint digest.

Structured requests enter a separate bounded source-ordered queue and are removed only after the
sole session owner accepts the exact answer. A full request queue uses the same staging rule, so a
later output cannot cross an unretained authority-bearing request. Normal provider closure and
typed drain failure both remove the exact worker through the ordinary bounded teardown path and
release only its token. Failures retain only a closed neutral class.

Canonical persistence is injected and idempotent by stable identity. Reusing an output identity
with unequal content is a persistence collision. For a paired event, output commits and is
checkpointed before activity begins. If activity fails, the accepted buffer item and its partial
checkpoint remain; retry may repeat the exact output and then completes activity. An item leaves
the buffer only after every required persistence effect and checkpoint succeeds.

The production persistence implementation lives in the node composition boundary. Pure
application planners author output as a correlated agent message and activity as the typed
`HarnessActivityRecorded` family. At the transaction snapshot, the adapter requires an active
unique local agent mailbox and either its exact unconflicted direct session binding or an exact
runnable project assignment binding. Local installation, human mailbox, agent mailbox, binding,
and complete prior activity-frontier evidence are causal support. Equal normalized replay has one
deterministic command identity; changed output identity reuse, equal activity sequence with changed
content, and stale binding reject without provider or store diagnostics.

## Environment and secret lifetime

Launch environment entries are bounded, copied into supervisor-owned memory, and independent of
the caller's source buffers. Names and values have per-entry and aggregate limits. Debug output is
redacted, cloning secret-bearing environment values is not exposed, and owned bytes are overwritten
on drop. Environment values are memory-only: they do not appear in ready state, leases, delivery or
event records, errors, reports, or persistence calls. Restart requires the caller to reconstruct a
launch environment from its authorized secret source.

The caller's absolute launch directory and copied environment enter only at application control.
The operation digest binds their exact normalized values, but the operation ledger persists neither
the directory nor environment. Canonical binding, repository context, and session selection occur
only after the provider acknowledges the exact ready session; exact resume must already name an
unconflicted binding to the same installation-local agent mailbox.

## Intake, cancellation, and shutdown

Application start, resume, stop, submission, structured-answer, and cancellation operations reach
provider sessions only through the sole supervisor owner. The adapter contract retains exact
operation cancellation semantics; cancellation never proves an uncertain delivery absent.

Interactive request retention requires at least one explicit responder registration. Registration
is idempotent by opaque identity. Arrival without a responder is answered fail-closed immediately;
removing the last responder answers every retained request fail-closed in source order. Provider
closure, worker stop, and shutdown use the same terminal path, so no request outlives its sole
session owner.

Shutdown is ordered and bounded for every worker even if a sibling fails:

1. stop adapter intake;
2. signal the component event task only after every live adapter has observed intake closure;
3. continue bounded ready drains so already accepted provider work reaches the FIFO or staging slot;
4. join the sole event task before moving or releasing the supervisor;
5. flush already accepted normalized events;
6. request adapter drain with the configured maximum wait;
7. force-stop when drain is pending or failed, and close the runtime capability idempotently; and
8. persist `Interrupted` successors for every projected `Running` agent turn in the exact owned
   provider session; and
9. release only the worker's exact lease token.

The supervisor report counts released and forced workers and retains only closed failure classes.
Pending accepted work or a persistence failure is reported; an interrupted provider is exactly
resumed and may replay normalized values through stable idempotent identities and durable partial
checkpoints. It is never silently treated as completed. The node lifecycle closes application
intake before invoking this sequence and maps any reported pending work or failure to escalation
evidence.

Exact resume performs the same reconciliation after acquiring the worker lease and before opening
the provider session. It loads only latest `Running` `AgentTurn` projections for that
agent/provider/session and advances each exact activity key by one semantic sequence with
`Interrupted`. A committed retry is naturally absent from the next query, so response loss and
repeated teardown cannot duplicate the terminal state. Already-terminal operations and other
provider sessions are never inferred or terminalized.

## Boundaries

`hq-harness` owns only provider-neutral synchronous records, state machines, and consumer-defined
ports. Passive config, requests, snapshots, and results expose public fields. Owner tokens, request
identities, secrets, and mutable runtime traits remain opaque because they enforce invariants or
grant capabilities. The crate imports no storage, serialization, async runtime, filesystem, process,
application, node, or provider-specific API.

`hq-store` owns SQLite rows and strict codecs. `hq-node` owns the record-only mapping, canonical
persistence adapter, and concrete component composition. Provider process and transport behavior belongs to private adapters. The
deterministic testkit exercises the same neutral supervisor and provider conformance contract used
by production adapters.
