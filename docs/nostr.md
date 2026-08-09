# HQ Nostr transport

Status: first-release transport for HQ canonical schema 1 and SQLite schema 5.

HQ uses Nostr relays to move already-signed canonical events between trusted installations. Relay events are transport records, not the source of HQ state. [events.md](events.md) defines the canonical event and reducer rules.

The implementation pins `fiatjaf.com/nostr` at revision `5fe6a7499d07` and keeps its NIP-44 use behind `internal/nostrwire`. HQ owns the rumor schema, gift-wrap validation, relay client interface, and durable sync rules.

## Address and keys

A network mailbox address is `(installation root public key, installation UUID, mailbox UUID)`. The Nostr root public key handles encryption, wrapper seals, and NIP-42 relay auth. The installation UUID remains stable application identity. A bare mailbox UUID has no network authority.

The local peer trust event binds a remote installation UUID to one root public key and up to three relay hints. Trust is one-way. The receiver must trust the sender before a remote canonical event can affect a projection. A peer may address the human mailbox by default; an agent mailbox also needs an active signed mailbox share.

## Wire layers

HQ sends one NIP-59 kind-1059 gift wrap per recipient installation:

1. The canonical HQ event remains signed kind 7281 exact bytes.
2. An unsigned kind-7282 rumor contains schema 1 `hq.canonical` JSON with the origin installation UUID, canonical event ID, and exact canonical event object. The rumor has one encrypted `p` tag for the recipient root key.
3. A sender-signed kind-13 seal contains the NIP-44 v2 encrypted rumor. Seal tags are empty.
4. A kind-1059 gift wrap contains the NIP-44 v2 encrypted seal. The gift wrap uses a fresh one-use key and one `p` tag for the recipient root key.

Seal and gift-wrap timestamps are independently moved to a random time in the prior two days. The canonical event keeps its signed application time.

HQ stores the exact signed gift-wrap bytes before the first publish. A retry sends the same bytes, event ID, timestamp, ciphertext, and one-use public key. HQ rejects one ephemeral public key used by two different wrappers.

## Relay-visible data

A relay can see the kind-1059 event ID, one-use public key, recipient root public key in the `p` tag, randomized time, wrapper size, and client network address. A NIP-42 auth exchange also shows the installation root public key to that relay. The relay cannot read the seal, canonical event, mailbox UUID, body, repository context, or causal links without a key compromise.

NIP-44 does not add forward secrecy or post-compromise security. A stolen root key can decrypt retained past and future wrappers for that key.

## Receive validation order

HQ applies checks in this order:

1. Enforce the 256 KiB wrapper input limit and strict JSON.
2. Verify the outer NIP-01 event ID and Schnorr signature before decryption.
3. Require kind 1059 and exactly one `p` tag for the local root public key.
4. NIP-44 decrypt the seal and verify its ID, signature, kind 13, and empty tags.
5. NIP-44 decrypt the rumor and require an unsigned kind 7282 whose public key matches the seal and whose `p` tag names the local root.
6. Parse the strict HQ envelope and verify the exact canonical event ID and signature.
7. Require the seal signer, envelope origin, canonical signer, canonical installation UUID, and local recipient installation to agree.
8. Apply local peer trust, signer binding, mailbox share, schema, size, route, and causal checks through the canonical reducer.
9. Insert the inbound wrapper, canonical event, projections, and dedup records in one SQLite transaction.

Bad signatures, MACs, recipients, schemas, sizes, identities, and rights enter bounded quarantine. Database lock and other temporary local failures enter staging. Quarantine does not retry on its own; an explicit recheck moves one row to staging.

## Relay sessions

HQ uses one WebSocket per relay during a sync pass. The client handles `EVENT`, `REQ`, `CLOSE`, `OK`, `EOSE`, `AUTH`, `CLOSED`, and `NOTICE` frames. Publish and subscription work share the connection.

Private inbox reads require NIP-42 by default. `hq relay add` enables auth. `--unsafe-no-auth` exists only for local development. A positive `OK` or a `duplicate:` response means relay acceptance. HQ keeps trying the other recipient relay hints after one relay accepts.

Catch-up starts a live kind-1059 subscription filtered by the local root `p` tag, handles stored events through EOSE, and pages older retained events with overlap. HQ deduplicates by outer Nostr event ID and by `(origin installation UUID, canonical HQ event ID)`. Gift-wrap times are random, so HQ does not use a narrow `since` cursor. NIP-77 remains optional future work.

Reconnect uses bounded exponential backoff. One unavailable relay does not remove queued work. Relay attempts, rejection text, retry times, auth state, EOSE time, and connection errors are unsigned node facts.

Each mutating CLI command commits its canonical event before a bounded foreground sync pass. `--no-sync` is the explicit offline switch. The stable stdout result, including the bare `hq ask` message ID, does not include sync status; a pending notice goes to stderr. `wait` performs bounded passes while it waits.

`hq daemon run` is optional. The daemon holds the sync lock, polls every 15 seconds, and accepts wake, status, and stop commands over a mode-0600 Unix socket next to the database. A lost wake does not lose work because the outbox stays durable and polling continues. The CLI still opens SQLite directly under WAL mode. Windows keeps the same service interfaces but does not yet expose local daemon control.

## Delivery terms

`queued` means HQ has durable canonical state and, when the peer key is known, durable exact gift-wrap bytes. `rejected` means a relay returned a negative `OK`; HQ keeps the retry schedule. `relay-accepted` means at least one recipient relay returned a positive or duplicate `OK`. `peer-received` requires a later valid causal child from the peer. Relay acceptance is not peer delivery.

HQ does not send automatic per-message receipts. A causal child proves receipt without extra writes. Batched receipt frontiers remain deferred.

## Sync lock

One process may own `hq.db.sync.lock` with a nonblocking advisory file lock. HQ never locks the SQLite database file. The lock limits duplicate relay work but does not provide correctness: event IDs, exact wrapper reuse, and SQLite uniqueness still make overlap safe. When the daemon owns the lock, a CLI process sends a wake request and returns. The interface leaves room for per-relay leases or database advisory locks later.

## First-release limits

HQ uses configured installation inbox relays and signed per-peer relay hints. Local bodies remain plaintext in SQLite, and same-user local actors are cooperative. NIP-44 provides neither forward secrecy nor post-compromise security. HQ does not publish kind 10050 relay lists, import generic kind-14 messages, use public events, rotate root keys, or run one installation identity on several active hosts.

## Human device pairing

Human account grants, acceptances, and revocations use the same exact-canonical-event and NIP-59 transport path as peer messages. The creator sends a grant to the invited installation. A copied pairing bundle lets the invited installation verify and import that grant even when relay delivery has not run. The invited installation sends its signed acceptance to the creator through the creator relay hints signed in the grant.

Pairing adds one-way peer trust on each installation only to carry these addressed events. The human account reducer still checks creator and device signatures, exact grant fields, and causal parents. Peer trust alone cannot create account membership. Account membership alone cannot address an agent mailbox.
