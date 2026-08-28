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
| canonical evidence transfer | 64 exact events and 524,288 aggregate event bytes |
| provider namespace | 64 bytes, nonempty |
| provider session | 256 bytes, nonempty |
| resource locator | 4,096 bytes, nonempty |
| relay policies in one status | 256, sorted and unique |
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
- bounded exact canonical-evidence closure query and reverified idempotent import;
- relay configuration and explicit synchronization effects, bounded relay/delivery status, domain
  health, and explicit repair;
- provider-neutral named-agent session control;
- exact node-owned named-agent retirement;
- read-only resource inspection;
- exact typed project control, including remote-home routing;
- subscription registration; and
- idempotent subscription cancellation.

Successful response families are lifecycle status, authoritative snapshot, conversation page,
mutation attempt, canonical evidence, evidence-ingest outcomes, empty external effect,
relay status, four-domain state health, explicit repair report, agent-session effect,
resource-inspection effect, typed project-command progress, named-agent-retirement progress,
subscription acknowledgement, and empty acknowledgement. Errors carry a closed class, stable code,
and optional bounded inert detail. Machine behavior depends on class/code, never detail text.

External effects retain their stable operation ID, exact request digest, issue time, and typed body.
Their result is `accepted`, `rejected`, or `uncertain`. An uncertain effect must be reconciled under
the same operation ID before retry; it is not silently translated into success or failure.

Named-agent retirement is an exact retryable request family rather than a generic effect or a
client-authored mutation. It carries command and operation identities, exact request digest,
authorizing account, agent, expected claim, immutable home, issue time, and explicit force policy.
Results are running, completed, rejected, or reconcilable; completion identifies an owning project
when assigned and retains optional succeeded, failed, or uncertain runtime truth. The reconnecting
client retains and replays the original encoded frame byte-for-byte and rejects changed reuse of a
command identity.

Relay status carries sorted current policies with access, authentication, enabled state, and
positive generation plus bounded delivery-state counts and an explicit truncation bit. State health
always carries one positive serialized revision and exactly the stable ordered authority,
conversation, agent, and project domain records. Repair requests carry a caller-selected 32-byte
operation identity; their report echoes it with the revision and the same complete domain catalog.
These are passive public-field DTOs. Because HQ has not shipped, the additions complete clean local
API v1 in place with no compatibility branch or version bump.

## Authoritative queries

An authoritative snapshot carries one serialized local revision and a bounded list of explicit
client projection DTOs. The closed projection union covers installation/mailbox/account authority,
peer routes, mailbox capabilities, device membership, account selection, conversation discovery,
named-agent lifecycle and provider-session registers, projects, accepted input, dispatch, output,
and remote-command progress. A remote-command item retains the complete request envelope and exact
request fact. Its progress is a closed `queued`, `received`, `terminal`, or `conflicted` value;
received and terminal values carry the exact receipt, observed head, and receipt time, while a
terminal value also carries the exact outcome fact, typed committed/rejected result, and optional
typed runtime observation. Project resources are separate projection items carrying display and
canonical locators, health, primary/active-claim flags, and bounded conflicting-project IDs. It is
a client query representation, not the reducer's Rust layout or
the store's normalized row schema.

Authority projections expose the exact public evidence needed for safe client-side planning:
installation items name their unique root fact, mailbox items name their creation fact, account
items name their unique creator root, and account-selection items name the complete causal-maximal
selection frontier. These are public fact identities, not signing capability. Because HQ v1 has
not shipped and has no standing installations, this is the clean v1 snapshot shape; it introduces
no compatibility branch or protocol-version bump.

Named-agent projections expose the same planning-grade public evidence without a signer or storage
handle. Agent items carry exact name-claim facts, candidate mailboxes, and retirement facts;
provider-session items carry every immutable binding fact; selection and display-name items carry
their causal-maximal candidates and complete frontiers. Context history retains each exact context
fact and typed value. These fields let a local client construct claim, exact selection, and
rename/clear plans while preserving conflicts instead of choosing by arrival or display order.

Membership items additionally carry their complete causal-maximal grant/accept/revoke frontier and
the complete creator-issued grant, usable acceptance, usable revoke, and active-acceptance history.
Each grant names its stable ID, exact signed fact, target installation and signing key, optional
label, bounded relay hints, and derived causal-frontier/active status. These passive fields let
clients preserve every membership maximum, expose ambiguity without selecting a historical winner,
attribute creator revocation to one exact grant, reuse the current unrevoked grant, build
frontier-complete regrants only after revocation, and verify exact target binding without
reconstructing storage rows.

Peer-route items carry the complete retained route-set and route-block histories. Each entry names
its exact fact and whether it belongs to the complete causal-maximal frontier; route sets also carry
the peer signing key, transport encryption key, optional label, and bounded relay hints. Validation
requires every frontier fact to be retained, the histories to be disjoint, every membership flag to
match the frontier, and `routable`/`blocked`/`conflicted` to agree with remove-wins derivation.

Mailbox-capability items carry the stable grant identity, exact grant fact, fully qualified mailbox
and grantee signing address, active flag, revoke frontier, observed action identities, and complete
support. Validation requires the grant and every revoke maximum in support and derives active state
from an empty revoke frontier. These are public planning facts, not signing or mailbox capability.
They complete the existing unshipped clean local API v1 shape in place, with no compatibility branch
or version bump.

The canonical-evidence query accepts sorted unique roots and returns their complete transitive
parent closure as sorted `(fact_id, exact_event)` values. Import accepts the same bounded shape,
cryptographically and semantically re-verifies the entire request before its first insertion, and
then uses ordinary idempotent canonical ingestion. Existing facts retain their original revision.
Neither operation grants authority; imported facts become usable only if ordinary complete
reduction projects their signed lineage.

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

The shared client retains the complete encoded mutation frame until a definite response. If a
response may have been lost, whether before or after commit, it repeats those bytes with the same
request and command IDs after renegotiation: it repeats the exact frame payload. The durable receipt
returns the original completed result. Reusing the command ID with any changed plan byte,
randomness byte, or digest is rejected
before transport. Completed command IDs and digests are retained in a configured bounded
oldest-first window; in-flight mutations are never evicted. A result is either a completed
committed/rejected receipt or explicit uncertainty; post-commit relay-wake failure does not change a
committed receipt.

A different command ID that authors the byte-identical canonical fact is a committed semantic
no-op. Its receipt names the fact's original revision, no new canonical revision or invalidation is
published, and later replay of that command returns its retained receipt. This permits independent
clients to race a deterministic reconciliation plan safely.

Project control uses a separate closed action DTO and the same stable command/digest discipline.
The request carries account, project, immutable home, expected head, operation, issue time, and the
complete typed action. The shared client retains its exact encoded frame across ambiguous response
loss. Accepted, running, and reconcilable progress may be explicitly resubmitted; completed and
rejected identities enter the same bounded completed-identity window. Changed digest reuse fails
before transport. The server delegates to `ControlProjects`; a non-home router authors only an
inert request fact, while the home executes the ordinary durable project workflow.

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

After reconnect, a client renegotiates and derives a new registration ID from its stable local
subscription seed plus the server's ephemeral session ID. It accepts the registration
acknowledgement's authoritative full snapshot as a fresh base and only then treats invalidations as
current. An invalidation marks the view stale and triggers a complete authoritative snapshot; it is
not a patch. Notices coalesce to their greatest revision while a refresh is in flight, and a
returned snapshot behind that revision immediately triggers another refresh. The client never
infers missed rows from a revision gap.

## Failure and close policy

Malformed, oversized, noncanonical, unknown-version, or out-of-state input closes the session after
any bounded typed response the session can safely produce. Application rejections use typed error
responses and keep the session usable. A stale socket or lost response provides no evidence about
mutation/effect completion; stable reconciliation rules apply.

The shared client scopes every transport event to a monotonically allocated local connection
generation and ignores late events from older sockets. Each connection starts with negotiation.
Explicit version rejection is terminal; ordinary disconnects use deterministic exponential backoff
bounded by configured positive minimum and maximum delays. Ordinary query/control requests are not
silently replayed after response loss: the client reports their request IDs as lost so the caller
can apply method-specific policy. Retry-safe mutations and project commands follow the exact-frame
rule above.
