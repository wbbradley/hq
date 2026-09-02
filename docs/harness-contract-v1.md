# Provider-neutral harness contract v1

Status: Normative Rust-era contract

This document defines the boundary between HQ and any managed runtime provider. The boundary is
provider-neutral: provider protocols, process layouts, credentials, diagnostics, and wire values
remain private to adapters. Normative terms such as MUST and MUST NOT describe required behavior.

## Ownership and identity

A logical instance is the independently failing runtime owner created for one named agent and an
optional project binding. Each call to `HarnessFactory::create_instance` MUST create independent
failure state. A crash in one instance MUST NOT corrupt, stop, or redirect a sibling instance.

A ready session is a durable provider-scoped conversation identity plus the sole mutable
`HarnessSession` owner. `Start` MUST create a new session and return only after the provider has
acknowledged a nonempty durable identity. `Resume` MUST open exactly the requested identity; it MUST
NOT create a replacement when that identity is absent. If an adapter acknowledges a different
identity, the registry force-stops the returned owner and rejects readiness.

The contract grants one mutable owner for submission, event draining, interactive responses,
cancellation, and shutdown. Sharing, leasing, persistence, and scheduling of that owner belong to
the supervisor, not the adapter contract.

## Capabilities and registration

`HarnessCapabilities.supported` is passive public data containing independently advertised
`HarnessCapability` values. Registration MUST reject duplicate provider namespaces. It MUST also
reject an adapter unless uncertain submission recovery is structurally safe through at least one of:

- `StableSubmissionIdempotency`: byte-for-byte repetition of one stable identity and digest cannot
  create a second provider operation.
- `SubmissionLookup`: lookup authoritatively distinguishes acceptance of the exact identity and
  digest from definite absence.

The registry MUST reject `Start` or `Resume` before instance creation when the corresponding
capability is absent. Advertisement is not behavioral proof; every adapter MUST also pass the
reusable conformance suite for its supported behavior.

## Submission and recovery

`HarnessSubmission.submission_id` is the stable HQ identity. Its `digest` covers the complete exact
neutral input. The operation identity correlates normalized events but is not a substitute for the
submission identity. Reusing one submission identity with another digest MUST fail closed as
`SubmissionIdentityConflict`.

A submission call returns one of three classes:

- `Accepted` means the exact identity and digest definitely entered provider ownership.
- `Rejected` means they definitely did not enter provider ownership.
- `Uncertain` means the call crossed a failure boundary after which acceptance is unknown.

An uncertain result MUST NOT be treated as rejection. If lookup is available, the caller first
looks up the same identity and digest. `Accepted` completes reconciliation without retry.
`Missing` permits one retry only with the same identity, digest, and body. An identity collision or
indeterminate lookup fails closed. A change in the provider's active-operation presentation during
recovery does not prove acceptance or absence and MUST NOT bypass reconciliation. If only stable
provider idempotency is available, a retry still uses the exact identity and digest.

Durable pending/uncertain/accepted/rejected ledgers, retry scheduling, and crash recovery are
supervisor responsibilities specified by `docs/harness-supervisor-v1.md`.

## Events and interactive requests

The adapter emits indivisible `HarnessEvent` values in provider source order. Before use, its sole
owner registers one `HarnessEventNotifier`; registration immediately signals already-ready input.
The adapter signals again when input changes from empty to ready, when backpressure release exposes
more input, and when the source closes. Repeated unconsumed signals may coalesce. After a signal the
owner calls `next_event` as a nonblocking bounded drain: `Pending` means no complete event is ready,
and `Closed` follows all preceding events after normal termination. The notifier is readiness only;
the event remains owned by the adapter until drained.

Output and activity use bounded domain values and typed fields. Output is actionable user-facing
content. Activity is non-actionable status/history and carries a positive semantic sequence.
Adapters MUST NOT require consumers to parse prose to determine output kind, activity kind, status,
correlation, or truncation. Provider event identifiers and transport messages remain private.

Interactive requests are bounded, structured, non-secret values with a stable opaque request ID.
They carry a closed question, command approval, file approval, permission, MCP URL, or MCP form
kind, exact operation identity, source-ordered stable choices, and explicit free-text capability.
Each request accepts at most one answer. Duplicate or unknown answers fail closed. Cancellation may
release an outstanding request and must use the adapter's method-specific closed response shape.
Passwords, tokens, authentication challenges, or other
secret-bearing input MUST NOT cross or be persisted at this neutral boundary; such a request returns
`SecretInputRejected` without retaining its content.

## Cancellation and shutdown

Cancellation targets one exact HQ operation and reports `Cancelled`, `AlreadyFinished`, definite
`Rejected`, or `Uncertain`. Cancellation is not evidence that an uncertain submission was absent.

Shutdown is explicit and ordered:

1. `stop_intake` idempotently rejects future submissions and interactive answers.
2. `drain` waits for at most its supplied duration and reports either `Complete` or bounded pending
   event and request counts.
3. `force_stop` idempotently terminates remaining adapter I/O or runtime ownership.

`Complete` means no accepted neutral event or interactive response remains inside the adapter. The
supervisor owns deadlines, escalation policy, buffer persistence, and external-process kill steps.

## Failure and trust boundary

`HarnessErrorClass` is a closed, stable control-flow classification. Neutral failures do not retain
provider prose, stderr, serialized payloads, credentials, or operating-system diagnostics. Adapters
may log redacted private diagnostics under their own policy, but callers MUST branch only on typed
outcomes and classes.

The neutral production crate MUST remain synchronous, object-safe, and runtime-independent. It may
depend on domain values, but not serialization frameworks, asynchronous runtimes, filesystem or
process APIs, provider SDKs, or provider vocabulary. The boundary does not authorize provider
actions; canonical HQ authority and application policy remain outside it.

## Deliberate exclusions

Worker leases, durable delivery/output ledgers, bounded FIFO and keyed coalescing, environment
copying/redaction, and node shutdown composition are defined by
`docs/harness-supervisor-v1.md`. Process spawning, transport framing, and operating-system kill
escalation remain private adapter mechanisms and do not leak into either neutral contract.
