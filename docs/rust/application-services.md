# Application services and ports

Status: normative first-release application boundary

`hq-application` owns use cases and the semantic values exchanged with adapters. It depends only on
`hq-domain` and `hq-reducer`. It contains no persistence, local framing, relay protocol, terminal,
filesystem, process, runtime, provider-specific, clock, randomness, or signing implementation.
Time, stable identities, request digests, causal references, and signing randomness enter as
explicit typed values.

## Authoritative query values

`ProjectionSnapshot<A, K, V>` owns ordered aggregate frontiers, typed projections, and exact support
sets. The four aliases cover authority/account/peer configuration, conversations and activity,
named agents and session selection, and projects. `DomainSnapshot` contains all four packages.
`AuthoritativeSnapshot` pairs them with the one local `Revision` at which they were read.

These values belong to the consumer boundary, not storage. A persistence adapter may re-export them
and reconstruct them through strict relational codecs, but callers never receive tables, SQL keys,
serialized reducer structs, or a persistence handle. Unified conversation pages contain only the
closed `ConversationEntry::{Message, Activity}` union and retain reducer-derived order and the
store-owned cursor.

## Consumer-owned ports

| Capability | Contract |
| --- | --- |
| `QueryDomain` | One revisioned authoritative refresh and bounded indexed conversation pages |
| `CommitFacts` | Execute or reconcile a stable transaction-consistent fact mutation |
| `PublishWake` | Nonblocking, coalescible prompt for post-commit replication/reconciliation work |
| `ConfigureRelays` | Stable relay-policy and explicit synchronization operations |
| `ControlHarness` | Neutral named-agent start, exact resume, and stop operations |
| `InspectResource` | Typed external observation without claiming a durable state transition |
| `ObserveRevisions` | Pending registration, later activation, and idempotent cancellation |

Ports name capabilities needed by the application. They do not mirror methods of a database,
socket, relay library, process client, filesystem library, or UI. The node composition root supplies
one `ApplicationPorts` bundle whose implementations may delegate to independently owned adapters.

`ApplicationError` has closed classes and codes. Adapters discard implementation prose and map
failures to invalid input, conflict, unauthorized, unresolved, not found, capacity, unavailable,
corrupt state, or invariant violation. Only later client and UI layers add safe presentation text.

## Fact-backed mutations and replay

`FactMutation` binds a 32-byte command ID to the digest of its exact request and a one-shot pure
decision. A commit adapter invokes that decision against a `DomainSnapshot` read inside the owning
transaction. The decision either rejects with a typed `DomainError` or returns a `FactPlan` made
only from explicit author, time, audience, causal and historical-authority references, semantic
payload, and BIP-340 auxiliary randomness. The protocol/storage adapter translates that plan into
canonical authoring and signing; application code never owns protocol DTOs or signer bytes.

The attempt result is one of:

- `Completed(MutationReceipt)` with an authoritative `Committed` or typed `Rejected` outcome; or
- `Uncertain { command_id, request_digest }` when a response may have been lost and exact replay is
  required before any new attempt.

The application-owned receipt encoding is canonical and versioned. A commit is bytes `01 00`. A
rejection is `01 01`, one closed error-category byte, a two-byte big-endian error-code length, and
the bounded UTF-8 stable error code. Unknown versions/categories, trailing data, invalid UTF-8,
length mismatch, noncanonical re-encoding, or disagreement with the store's committed/rejected kind
is corrupt state. Rust layouts, transport results, diagnostic prose, and secrets are never retained.

After a committed receipt, `Application::execute_mutation` separately calls `PublishWake`. The
returned `MutationCompletion` keeps the receipt authoritative and reports scheduling as
`Scheduled`, `Coalesced`, or a typed error. A wake failure cannot turn a committed command into a
failure and cannot justify retrying it under a new identity. Rejections and uncertain attempts do
not schedule work.

## External operations

`EffectRequest<T>` carries a stable `OperationId`, exact request digest, explicit issue time, and
typed body. Relay configuration, synchronization, neutral agent-session control, and resource
inspection each retain their own capability trait because their reconciliation rules differ. Their
common `EffectOutcome<T>` is `Accepted`, typed `Rejected`, or `Uncertain(operation_id)`. Uncertainty
means reconcile that identity before repeating the effect; persisted intent alone is never accepted
evidence that external work happened.

Relay configuration contains only a typed endpoint locator, read/write policy, and authentication
policy. It contains no credentials or client-library values. Session control names a durable agent,
neutral provider namespace, and start/exact-resume/stop action. Resource inspection names the
project, resource, display locator, and recorded canonical locator. It returns only bounded inert
details, typed health, an optional newly observed canonical locator, and an explicit observation
time. These passive request/result values expose fields directly. Later workflow owners turn
accepted observations into canonical facts.

Project workflow intake uses one public passive `ProjectCommandRequest` with stable command and
operation identities, an exact digest, account/project/home identities, expected project head,
explicit issue time, and a closed `ProjectCommandAction`. Results are typed accepted, running,
completed, rejected, or reconcilable outcomes with an explicit durable checkpoint. The
`ControlProjects` capability is opaque because its implementation owns project serialization and
bounded recovery; the request, action payloads, provisioning request, and outcome fields are not
hidden behind accessors.

## Subscription revision race

Subscription preparation has three ordered phases:

1. `ObserveRevisions::register_subscription` creates a pending observer.
2. `QueryDomain::authoritative_snapshot` reads the revision and all projection packages.
3. The caller writes that snapshot acknowledgement, then separately invokes
   `activate_subscription`.

If snapshot loading fails, the service cancels the pending observer before returning the query
error. Preparation never activates delivery. This lets the local-session adapter buffer/coalesce
changes after registration without delivering before its acknowledgement has been written. Active
or pending registration cancellation is idempotent.

## Store adaptation and acceptance

`hq-store::StoreGateway` is configured with an explicit `AuthorityPolicy` and a shared signer
capability. It implements only `QueryDomain` and `CommitFacts`; the node combines it with separately
owned relay, runtime, resource, and observation adapters. The store actor loads revision and all four
projection packages in one serialized request. Mutation decisions enter the existing atomic local
commit path, and retained result bytes are strictly decoded back into application receipts.

Contracts prove exact replay does not decide twice, changed-digest reuse conflicts before decision,
pure rejection and commit translation, post-commit wake independence, accepted and uncertain
external outcomes, registration/query/activation order, query-failure cancellation, and store
gateway equality. The architecture verifier forbids runtime and adapter concerns in
`hq-application` and permits the dependency only from adapters toward it.
