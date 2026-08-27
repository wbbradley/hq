# Relay synchronization v1

Status: Normative for the Rust rewrite

Relay synchronization moves exact prepared Nostr envelopes between installation nodes. Relays are
retained, untrusted transport services: connection state, event order, `EOSE`, timestamps, `OK`
responses, and NIP-42 authentication never grant application authority or select domain winners.
Only an opened envelope's exact raw canonical bytes enter the ordinary verification and reducer
path described by [Nostr envelope v1](nostr-envelope-v1.md).

## Ownership and ports

The node owns one relay manager and exactly one session state machine per enabled relay URL. The
`hq-relay` crate owns transport records and consumer-side traits for durable state, route
resolution, canonical ingest, clocks, sleeping, and connections. It contains no SQLite or
projection-table types. `hq-store` independently owns strict durable record types and SQLite
transactions. Only `hq-node` maps between those boundaries.

One session exclusively owns its connection, latest NIP-42 challenge, subscriptions, retry clock,
and in-flight writes. A capacity-one wake is a work notification, not work itself. Durable state is
always re-read after a wake, periodic repair tick, reconnect, or restart.

Every independently ordered durable collection uses a typed keyset position: start, strictly after
one stable key, or done. A page marks an exhausted collection done while other collections continue,
so work after the first 1,024 rows remains reachable without restarting completed scans. Only the
lexicographically first enabled session owner retries global staging; inbound delivery on every
session still uses the same atomic claim and quarantine transitions.

## Relay URLs and policies

A relay URL is at most 2,048 ASCII bytes, begins with lowercase `ws://` or `wss://`, has a non-empty
host authority with an optional numeric port, contains no credentials, fragment, ASCII whitespace,
or controls, and may carry a path/query. Bracketed IPv6 authority is retained exactly. Exact
spelling is its durable identity; HQ performs no lossy URL normalization.

The production connector is a blocking Tungstenite adapter with Rustls/native roots at this outer
I/O boundary only. Configuration bounds TCP/TLS/WebSocket handshake time, redirects, complete
message and frame bytes, failed-write buffering, and each socket write. Every receive call installs
the caller's remaining deadline on the underlying plain or Rustls socket; ping/pong is handled by
the WebSocket protocol engine, binary application messages are rejected, and close is typed.

A policy contains URL, read/write access, authentication mode, enabled state, and a positive
monotonic generation. Configuration operations have a stable 32-byte operation ID and request
digest. Equal replay is idempotent. Reusing an operation ID with unequal input conflicts. A changed
policy allocates the next generation atomically. Ordinary work wakes do not change the generation.

A session is recreated only after connection failure, lifecycle restart, or a relevant policy or
authentication generation change. Disabling/removing a relay stops new work and drains its owner;
it does not delete accepted audit history, prepared wrappers, or canonical facts.

## Outbound durable states

Canonical ingest creates one queued outbox intent per recipient. Human-device grants and revokes
always include their explicitly named device even when that device is not currently active; this is
the transport path by which pairing or removal reaches the subject. Route resolution is a separate
read of already-verified signer/routing state immediately before first preparation. A relay cannot
write or override a route.

For one `(canonical event ID, recipient installation)` lineage:

1. `queued`: exact canonical bytes and recipient intent are durable.
2. `prepared`: exact kind-1059 bytes and all envelope-v1 metadata are durably committed with the
   one-use public-key uniqueness claim before any `EVENT` frame.
3. `attempted`: a relay-local attempt count/time and next retry deadline are durable. Sending may
   already have happened; loss before `OK` is uncertain and retries the same exact bytes.
4. `relay-accepted`: one relay returned positive `OK` or a machine-readable duplicate response.
   Other relay rows remain audit evidence; policy may continue attempting eligible hints.
5. `relay-rejected`: one relay returned negative `OK`; its redacted reason class and retry state are
   durable. Rejection by one relay does not reject the canonical fact or other relays.

Relay `OK` text is never durable. Prefixes are reduced immediately to the closed
`authentication-required`, `rate-limited`, or `permanent` classes. Positive `OK`, including a
duplicate acknowledgement, is absorbing. Rate-limited and authenticated retry always reuse the
already prepared exact bytes.

Prepared wrapper ID and one-use public key are globally unique. Equal insertion is idempotent;
unequal reuse is corruption/conflict. Preparation plus the uniqueness claim is one transaction. A
crash exposes either queued work or the entire prepared lineage. No retry re-encrypts, re-signs, or
changes bytes/timestamps.

Relay acceptance is not peer receipt. Only a later authorized causal peer event can establish
`peer-received` in domain reduction.

## Catch-up and live edge

A readable session first opens a live kind-1059 subscription filtered to the local root `p` tag,
then pages retained events backward. Live events arriving during catch-up are buffered within the
session bound and processed through the same durable deduplication transition.

Gift-wrap timestamps are randomized across the preceding two days, so a narrow recent-time `since`
cursor is forbidden. Each retained page uses an inclusive `until` boundary, a deterministic event-ID
tie boundary, and overlap. The durable cursor records the active scan-start wall time, the latest
fully covered scan-start time, the oldest observed `(created_at, outer ID)`, exhaustion, and policy
generation. Resume repeats the boundary page for every unfinished scan. A new connection starts a fresh head scan
after an exhausted scan; its live filter and backward traversal overlap the previous coverage by the
full two-day randomization range plus one second. If a crash resumes an unfinished old scan, the
session finishes it and then starts the connection's fresh head scan before releasing buffered live
events. Outer-ID and logical-ID deduplication make overlap, relay duplicates, disconnect, and restart
harmless. A policy generation change may restart catch-up but never erases deduplication evidence.

Portable NIP-01 filters have no event-ID range operator. If an inclusive full page does not produce
a strictly older `(created_at, outer ID)` boundary, HQ closes that page, retains it as unexhausted,
and retries the same inclusive boundary with capped backoff. It never skips a possibly unbounded
same-second tie or falsely declares exhaustion. An initial scan exhausts only on a short page. A
later refresh may also finish after a page crosses strictly below the prior coverage-overlap floor;
older history was already proven by the preceding completed scan.

`EOSE` closes only the named retained page or live catch-up phase. It does not certify completeness,
authorize data, or advance domain state. Catch-up reaches exhaustion only through the documented
bounded backward-page rule.

## Inbound transitions

Complete outer input is bounded before parsing and opened according to envelope v1. A successful
open yields exact canonical bytes plus non-authoritative audit metadata. The common canonical ingest
port re-verifies those bytes and returns one of:

- committed/already present: atomically claim the outer wrapper ID and logical
  `(origin installation ID, canonical ID)` identities;
- transient local failure: store exact outer bytes in staging with bounded attempts and retry time;
- permanent transport/canonical failure: store only bounded quarantine evidence.

Outer and logical claims are one transaction. Equal duplicates are no-ops even when a later relay
observation has a different receive time; the first receive time is retained. One outer ID mapping to
different logical data, or one logical identity mapping to unequal canonical evidence, fails closed.
Relay URL/order/time/acceptance is audit input only and is not passed to reduction.

## Staging and quarantine bounds

- Each collection in an outbox/cursor/state query page: 1 through 1,024 records.
- Exact prepared or staged outer wrapper: 1 through 262,144 bytes.
- Quarantine raw sample: at most 4,096 bytes.
- Relay acknowledgement, notice, or authentication challenge: at most 1,024 bytes.
- Staging collection: at most 1,024 rows and 64 MiB exact outer bytes.
- Quarantine collection: at most 1,024 rows and 4 MiB samples.
- Attempt count: unsigned 32-bit; exhaustion remains permanently staged with a redacted class until
  operator policy or later explicit repair handles it.

Staging is FIFO by first-received time then wrapper digest and evicts nothing automatically: when
full, intake backpressure/disconnect preserves relay-retained input for later catch-up. Successful
ingest removes the staged row in the same transaction that records deduplication.

Quarantine is diagnostic and may evict. Before commit it deterministically removes oldest rows by
receive time then digest until both count and sample-byte limits hold. It stores receive time,
complete byte length, SHA-256 digest, verified outer ID when available, redacted failure class, and
the bounded raw outer prefix. It never stores decrypted rumor/seal/canonical plaintext or secrets.

## Authentication, retry, refresh, and shutdown

The latest NIP-42 challenge is connection-local and replaces any earlier challenge. Required mode
authenticates before ordinary work; on-challenge mode responds when requested or after an
`auth-required` response. Authentication events use the installation root key and exact active URL
and challenge, but are neither published facts nor durable authorities.

Reconnect and outbound retry use deterministic capped exponential backoff with injected monotonic
time and jitter. A new durable work wake coalesces with an existing wake and never tears down a
healthy subscription. Periodic polling repairs a missed wake.

Shutdown closes intake, checkpoints uncertain in-flight writes as retryable attempts, closes named
subscriptions/connections, and joins every session owner. Prepared bytes, staging, cursors, and
deduplication survive. Forced escalation may terminate I/O only after uncertainty is durable.

## Acceptance traces

The deterministic scripted relay must cover retained pages, overlapping boundaries, a live event
during catch-up, duplicates, shuffled delivery, `EOSE`, disconnect, publish response loss,
positive/duplicate/negative `OK`, challenge replacement, `auth-required`, missed/coalesced wakes,
policy refresh, staging recovery, quarantine eviction, restart, and drain.

Two distinct installation fixtures must then prove equal verified fact sets and projections after
arbitrary delivery order and downtime, including relay restart and authorization revoke/regrant
traffic. A separately opt-in controlled retained-relay smoke checks real WebSocket/NIP-42/catch-up
interoperability; external network availability is not a unit or merge gate.
