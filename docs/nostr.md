# HQ Nostr transport

Nostr is HQ's remote transport between installation nodes. It is not local IPC and it is not the
source of application state. Local CLI, TUI, and Codex clients use versioned domain RPC; the node
signs and stores canonical events, then its continuous network engine moves exact encrypted wrappers
through retained relays.

This release uses canonical schema 1 and SQLite schema 10. The implementation pins
`fiatjaf.com/nostr` at revision `5fe6a7499d07` behind `internal/nostrwire`; HQ owns the canonical
schema, wrapper validation, durable outbox, relay interface, and retry rules.

## Addresses and authority

A remote mailbox address is `(installation root public key, installation UUID, mailbox UUID)`. The
root key signs canonical events, opens NIP-44 envelopes, and authenticates to NIP-42 relays. The
installation UUID is stable application identity. A bare mailbox UUID grants no authority.

One-way peer trust binds a remote installation UUID to one public key and up to three relay hints.
The receiver must trust the origin before peer traffic can project. A peer may address the reserved
human mailbox; an agent mailbox also needs an active signed share. Human-account traffic uses signed
device grants and account authority rather than mailbox sharing.

## Exact wire layers

HQ creates one NIP-59 kind-1059 gift wrap per recipient installation. Peer-addressed events have one
recipient. Account-addressed events have one canonical event and one durable outbox row/wrapper per
active device.

1. Exact signed kind-7281 bytes contain the canonical HQ event.
2. An unsigned kind-7282 rumor contains schema-1 `hq.canonical` JSON with origin installation UUID,
   canonical ID, and the exact canonical event. Its `p` tag names the recipient root key.
3. A sender-signed kind-13 seal contains the NIP-44 v2 encrypted rumor and has no tags.
4. A kind-1059 gift wrap contains the encrypted seal, a fresh one-use key, and one recipient `p`
   tag.

Seal and wrapper timestamps are independently randomized into the prior two days. The canonical
event retains its signed application time. Before first publish, the node durably stores the exact
gift-wrap bytes, ID, ciphertext, timestamp, and one-use key. Every retry reuses those exact bytes.
HQ rejects an ephemeral key reused by different wrappers.

## Relay-visible data

A relay sees the kind-1059 ID, one-use public key, recipient root key, randomized time, wrapper size,
and client network address. NIP-42 also exposes the installation root public key to that relay. The
relay cannot read the seal, canonical event, mailbox UUID, body, repository context, or causal
links without a key compromise. NIP-44 provides neither forward secrecy nor post-compromise
security.

## Node-owned relay sessions

The node's continuous engine derives write relays from signed peer hints and read/write relays from
local configuration. It uses one WebSocket per relay for `EVENT`, `REQ`, `CLOSE`, `OK`, `EOSE`,
`AUTH`, `CLOSED`, and `NOTICE`. Private inbox reads require NIP-42 unless the explicit development
`--unsafe-no-auth` override is configured.

Catch-up opens a live kind-1059 subscription filtered by the local root `p` tag, consumes retained
events through EOSE, and pages older events with overlap. Random wrapper times deliberately rule out
a narrow `since` cursor. HQ deduplicates outer IDs and `(origin installation UUID, canonical event
ID)`, so overlap, relay duplicates, restart, and catch-up are safe.

The same session publishes ready outbox jobs. A positive `OK` or `duplicate:` response records relay
acceptance; a negative response preserves retry state. Other relay hints remain eligible after one
accepts. Connection loss uses bounded exponential backoff, and durable queued work survives node,
host, or relay downtime. Config changes and lifecycle wake requests refresh sessions immediately;
periodic polling repairs a missed wake.

`hq sync` asks the node to wake its engine and run promptly; it does not make the CLI a relay worker.
Normal mutating commands may request the same immediate synchronization after their local commit.
Global `--no-sync` only suppresses that client request/wait. It is not an offline guarantee: an
already running, network-enabled node can still publish durable outbox work. A true node-wide
offline mode would be a separate future feature.

## Inbound validation and commit

The node validates in this order:

1. Enforce the 256 KiB input limit and strict JSON.
2. Verify outer NIP-01 ID/signature, kind 1059, and exactly one local recipient `p` tag.
3. Decrypt and verify the kind-13 seal, signer, ID, signature, and empty tags.
4. Decrypt the unsigned kind-7282 rumor and verify its author and local recipient tag.
5. Parse the strict HQ envelope and verify exact canonical ID/signature bytes.
6. Require seal signer, envelope origin, canonical signer, and installation UUID to agree.
7. Require a direct local route or active membership in the named human account.
8. Apply peer/account authority, signer binding, mailbox rights, schema, size, and causal rules.
9. In one SQLite transaction, insert wrapper audit data, append the canonical event, reduce,
   project, derive any new outbox work, increment the revision, and commit.
10. Publish a lightweight local invalidation after commit.

Local and inbound canonical events therefore take the same reducer and projection path. Bad
signatures, MACs, recipients, identities, schemas, sizes, or rights enter bounded quarantine.
Temporary database failures enter staging for retry.

## Delivery states and recovery

`queued` means canonical state and recipient work are durable; once prepared, exact wrapper bytes
are durable too. `rejected` means a relay returned a negative `OK` and retry state remains.
`relay-accepted` means one relay accepted or already retained the wrapper. `peer-received` requires
a later valid causal event from the peer. Relay acceptance alone is not peer delivery.

The node never relies on relay receipt order for semantic reduction. Canonical IDs, mutation
receipts, exact wrapper reuse, uniqueness constraints, inbound wrapper audit rows, and logical
deduplication provide exact-once projection across response loss, concurrent retry, restart, and
retained catch-up.

## Human device traffic and limits

Human grants, acceptances, revocations, questions, answers, and archive facts use the same exact
canonical/NIP-59 path. Pairing bundles include signed authority history so initial verification does
not depend on relay arrival order. Peer trust added during pairing enables direct traffic but cannot
create account membership.

HQ does not publish kind-10050 lists, import generic kind-14 messages, use public events, rotate root
keys, provide forward secrecy, or support one installation identity on multiple active hosts.
[lan.md](lan.md) describes the pinned retained relay and two-node operation.
