# HQ local API v1

Status: normative for the Rust first release.

Owner: `hq-local-api::protocol::v1`.

## Purpose and trust boundary

Local API v1 is the only client protocol for the CLI, TUI, and local harness launchers. It carries
lifecycle control, application queries, stable commands, external-effect requests, and revision
invalidations over a same-user local byte stream. It has an independent version space and does not
reuse the canonical-fact, remote-control, Nostr-envelope, harness, or conformance-trace versions.

The local transport authenticates only the operating-system user selected by the node. A local
message grants no domain authority. Domain authority still comes from ordinary verified canonical
facts, historical authorities, and reducer policy. Clients never receive a storage handle, signing
key, relay client, provider process, or filesystem adapter.

Protocol DTOs are defined only by `hq-local-api`. Rust domain, reducer, application, storage, and
node layouts are not wire schemas and do not derive their local API representation.

## Framing and canonical encoding

Every message is one four-byte unsigned big-endian body length followed by exactly that many UTF-8
JSON bytes. The declared body length is checked before body buffering or JSON decoding.

The body is the compact serialization produced by the v1 DTO codec:

- no insignificant whitespace;
- DTO declaration order for object fields;
- exact lowercase `snake_case` enum tags;
- no unknown, missing, duplicate, or trailing fields;
- no trailing bytes after the declared body; and
- re-encoding the decoded DTO must reproduce the body byte for byte.

Semantically equivalent noncanonical JSON is rejected. A stream decoder may retain a partial
frame and bytes belonging to a later frame, but retains at most one maximum-size frame plus its
four-byte prefix.

## Bounds

All limits are inclusive. A larger value is rejected before it can become application input.

| Value | v1 maximum |
| --- | ---: |
| JSON body | 1,048,576 bytes |
| incremental frame buffer | 1,048,580 bytes |
| build name, version, or commit field | 128 bytes, nonempty, no control characters |
| unsigned canonical mutation plan | canonical v1 `MAX_CONTENT_BYTES` |
| conversation page | 256 entries |
| opaque page cursor | 512 bytes, nonempty |
| invalidation topics | 6, nonempty, sorted, unique |
| authoritative snapshot | 16,384 projection items |
| provider namespace | 64 bytes, nonempty |
| provider session | 256 bytes, nonempty |
| resource locator | 4,096 bytes, nonempty |
| short names, states, categories, and error codes | 128 bytes, nonempty |
| content and inert diagnostic detail | 16,384 bytes, nonempty when present |

Fixed-width identities, public keys, digests, and auxiliary randomness are exactly 32 bytes. A
request ID is a nonzero unsigned 64-bit integer. Activity and project-attribution sequences are
positive. A revision invalidation names a positive committed revision.

## Connection and negotiation

The first client message is `client_hello`, containing an inclusive nonzero ordered version range
and diagnostic build metadata. The server replies with exactly one of:

- `server_hello`, selecting the highest common version and naming an ephemeral connection-session
  identity; or
- `version_rejected`, naming its supported range, then closes the connection.

No request, response, or invalidation is valid before `server_hello`. Build metadata is diagnostic
only; it never changes version selection or authorizes behavior. Each new connection, including a
reconnect after node restart, negotiates again.

## Message catalog

The top-level closed message union is:

| Direction | Message | Meaning |
| --- | --- | --- |
| client to server | `client_hello` | version/build offer |
| server to client | `server_hello` | successful negotiation |
| server to client | `version_rejected` | incompatible versions and close |
| client to server | `request` | nonzero request ID plus typed method |
| server to client | `response` | same request ID plus success or typed error |
| server to client | `invalidation` | active subscription wake only |

Request methods are closed and typed:

- lifecycle `status`, `readiness`, `stop`, and `restart`;
- authoritative snapshot;
- bounded reducer-ordered conversation page;
- exact retryable canonical-fact mutation;
- relay configuration and explicit synchronization effects;
- provider-neutral named-agent session control;
- read-only resource inspection;
- subscription registration; and
- idempotent subscription cancellation.

Successful response families are lifecycle status, authoritative snapshot, conversation page,
mutation attempt, empty external effect, agent-session effect, resource-inspection effect,
subscription acknowledgement, and empty acknowledgement. Errors carry a closed class, stable code,
and optional bounded inert detail. Machine behavior depends on class/code, never detail text.

External effects retain their stable operation ID, exact request digest, issue time, and typed body.
Their result is `accepted`, `rejected`, or `uncertain`. An uncertain effect must be reconciled under
the same operation ID before retry; it is not silently translated into success or failure.

## Authoritative queries

An authoritative snapshot carries one serialized local revision and a bounded list of explicit
client projection DTOs. The closed projection union covers installation/mailbox/account authority,
peer routes, mailbox capabilities, device membership, account selection, conversation discovery,
named-agent lifecycle and provider-session registers, projects, accepted input, dispatch, output,
and remote-command progress. It is a client query representation, not the reducer's Rust layout or
the store's normalized row schema.

Conversation bodies and activity are loaded through the bounded page method using a typed thread or
provider-session conversation key. The continuation cursor is opaque, belongs to this query, and is
never interpreted by a client. Page order is the reducer's canonical presentation order.

## Stable mutation retry

A mutation contains:

1. a stable 32-byte command ID;
2. a 32-byte exact request digest;
3. canonical v1 unsigned semantic-plan content; and
4. 32 bytes of auxiliary signing randomness.

The plan content uses the canonical protocol owner's exhaustive semantic DTO codec so all fact
families have one semantic spelling. It is deliberately unsigned and is not a fact or admissible
evidence. The node strictly decodes it, constructs an application `FactPlan`, and only the node's
ordinary signer plus canonical verification can produce a fact.

The request digest is SHA-256 over this exact byte sequence:

```text
"hq-local-api-v1-mutation\0"
u32_be(plan_content_length)
plan_content
auxiliary_randomness
```

The client retains the complete mutation bytes until a definite response. If a response may have
been lost, it repeats the exact frame payload with the same command ID. The durable receipt returns
the original completed result. Reusing the ID with any changed plan byte, randomness byte, or digest
is a conflict. A result is either a completed committed/rejected receipt or explicit uncertainty;
post-commit relay-wake failure does not change a committed receipt.

## Revision subscriptions

Topics are broad: `all`, `authority`, `conversation`, `agent`, `project`, and `operations`. A
notification contains only subscription ID, newest committed revision, sorted topics, and a
`full_snapshot` flag. It never contains projection rows, message bodies, secrets, or transport
observations.

The server registers a subscription as pending before reading the authoritative snapshot named by
its acknowledgement. The acknowledgement contains that snapshot. The subscription becomes active
only after the acknowledgement frame is confirmed written. Disconnect cancels pending or active
registration idempotently.

A server session accepts at most one unconfirmed response write. The opaque confirmation ticket is
owned by that session, is single-use, and cannot activate another session's registration. The next
request is not routed until the prior response is confirmed, bounding retained write state and
preventing response-loss ambiguity from accumulating side effects.

Each active subscriber has one nonblocking coalescing wake slot. New wakes union broad topics,
retain the greatest revision, and set `full_snapshot` if any coalesced wake requires it. A slow or
nonreading subscriber never blocks a commit.

After reconnect, a client renegotiates, registers a new subscription, accepts its authoritative full
snapshot, and only then treats later invalidations as current. It never infers missed rows from a
revision gap.

## Failure and close policy

Malformed, oversized, noncanonical, unknown-version, or out-of-state input closes the session after
any bounded typed response the session can safely produce. Application rejections use typed error
responses and keep the session usable. A stale socket or lost response provides no evidence about
mutation/effect completion; stable reconciliation rules apply.
