# Provider-neutral harness supervisor v1

Status: Normative Rust-era operational contract

This document defines the consumer-owned supervisor above the provider-neutral harness contract.
The supervisor coordinates external effects for one installation; its leases and ledgers are
operational state, not canonical facts and not authority.

## Worker ownership and readiness

There is at most one mutable worker owner for each named agent. A worker owns one provider session,
one fixed-capacity event buffer, and one opaque 32-byte owner token. Before opening a provider
session it MUST atomically claim the agent's durable lease. A live exact owner may renew its lease.
A different token may take over only after the injected absolute deadline has expired.

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

Canonical persistence is injected and idempotent by stable identity. Reusing an output identity
with unequal content is a persistence collision. For a paired event, output commits and is
checkpointed before activity begins. If activity fails, the accepted buffer item and its partial
checkpoint remain; retry may repeat the exact output and then completes activity. An item leaves
the buffer only after every required persistence effect and checkpoint succeeds.

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

Shutdown is ordered and bounded for every worker even if a sibling fails:

1. stop adapter intake;
2. flush already accepted normalized events;
3. request adapter drain with the configured maximum wait;
4. force-stop when drain is pending or failed, and close the runtime capability idempotently; and
5. release only the worker's exact lease token.

The supervisor report counts released and forced workers and retains only closed failure classes.
Pending accepted work or a persistence failure is reported and remains durable; it is never silently
dropped. The node lifecycle closes application intake before invoking this sequence and maps any
reported pending work or failure to escalation evidence.

## Boundaries

`hq-harness` owns only provider-neutral synchronous records, state machines, and consumer-defined
ports. Passive config, requests, snapshots, and results expose public fields. Owner tokens, request
identities, secrets, and mutable runtime traits remain opaque because they enforce invariants or
grant capabilities. The crate imports no storage, serialization, async runtime, filesystem, process,
application, node, or provider-specific API.

`hq-store` owns SQLite rows and strict codecs. `hq-node` owns the record-only mapping and concrete
component composition. Provider process and transport behavior belongs to private adapters. The
deterministic testkit exercises the same neutral supervisor and provider conformance contract used
by production adapters.
