# HQ canonical event protocol

Status: first-release protocol, schema version 1.

HQ derives durable state from signed canonical events. SQLite tables, relay queues, and user views are indexes of those events. A supported HQ command must not change durable domain state without creating and applying a valid event.

The format follows [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md) for event IDs and Schnorr signatures. [nostr.md](nostr.md) defines the relay wrapper. Relay URLs and transport retry data are not part of a canonical event.

## Identity

Each installation has:

- A stable random UUID called the installation ID.
- One root secp256k1 key in the first release.
- Any number of mailbox UUIDs.

A logical human account has its own stable UUID and may grant several installation identities the right to act as devices for that human. The account UUID is not a mailbox address, installation ID, or public key.

The installation ID does not change when a key changes. Schema version 1 does not implement key rotation, but every event contains `signer_key_id` so a later reducer can check signed key grants. The first-release key ID is the root public key as 32-byte lowercase hex.

A full mailbox address is `(installation_id, mailbox_id)`. A bare mailbox UUID is not a network address and grants no access.

## Nostr envelope

HQ uses provisional regular Nostr kind `7281`. The project may register or change this kind before HQ 1.0. The Nostr `tags` array is empty in schema version 1. All HQ fields live in `content` as compact JSON.

HQ computes the Nostr event ID from the NIP-01 serialization and signs that ID with BIP-340 Schnorr over secp256k1. A receiver must check the content-derived ID and signature before it parses or applies HQ content.

The full wire event may not exceed 65,536 bytes. HQ stores the exact received bytes for audit and retry, while the Nostr event ID remains the logical deduplication key.

## Common content

The Nostr `content` string contains this object in field order when HQ creates an event:

```json
{
  "schema": 1,
  "type": "question",
  "installation_id": "0198c7ec-73b0-7cc3-a5f7-e31c77140d01",
  "signer_key_id": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
  "sender": {
    "installation_id": "0198c7ec-73b0-7cc3-a5f7-e31c77140d01",
    "mailbox_id": "0198c7ec-73b0-7cc3-a5f7-e31c77140d11"
  },
  "audience": {
    "human_account_id": "0198c7ec-73b0-7cc3-a5f7-e31c77140d21"
  },
  "parents": ["a signed account membership event ID"],
  "scope": "account-addressed",
  "payload": {
    "body": "Which port should I use?"
  }
}
```

Fields have these rules:

- `schema` is a positive protocol version. Version 1 uses strict JSON fields.
- `type` selects one payload schema and reducer rule.
- `installation_id` names the installation that created the event. A message sender must belong to that installation.
- `signer_key_id` must equal the Nostr event public key until key grants exist.
- `sender` is present on message events. A direct route has at most one `recipient`. An account question omits `recipient` and names a human account in `audience`.
- `thread_id` is present on child thread events and absent on a root question or async message.
- `parents` is a set of causal event IDs in lexical order. It may contain at most 64 IDs and no duplicate.
- `scope` is `installation-private`, `peer-addressed`, or `account-addressed`. Account-addressed events require a matching human-account audience. `public` is reserved and rejected in the first release.
- `origin` may name a source installation and event for an import or forward. An origin is not a causal parent.
- `payload` is a strict object selected by `type`.

Event bodies and details must contain valid UTF-8. A body may not exceed 32,768 bytes. Details may not exceed 16,384 bytes. Schema version 1 does not carry files or binary data.

Local message payloads also carry a stable message UUID and an immutable repository-context snapshot. The UUID is the short user-facing handle accepted by `get`, `wait`, `answer`, and `cancel`. The Nostr event ID remains the signed deduplication key and causal reference. Remote protocol work may change the handle format before HQ 1.0.

## Threads and causal links

Canonical events form a directed acyclic graph through `parents`. HQ defines no installation-wide or global sequence.

A root `question` or async `message` cannot include its own content-derived event ID: doing so would make the ID depend on itself. The root therefore omits `thread_id`, and the reducer derives its thread ID from the root event ID. Every answer and thread state event names that derived ID.

A child may list more than one parent. Parent sets can carry as much causal context as the sender knows. A signed timestamp helps people read concurrent events, but it does not resolve state conflicts. Local receipt time is diagnostic data only.

Events may arrive before a parent. HQ retains a valid and authorized child as `unresolved`, seeks the missing parent, and does not invent the unseen thread state. An unresolved addressed message remains visible through mailbox polling and direct lookup.

## Event types

| Type | Scope and payload | Effect |
| --- | --- | --- |
| `installation.create` | Installation-private; optional `label` | Records installation creation. |
| `mailbox.create` | Installation-private; `mailbox_id`, `kind`, optional `label` | Creates a human or agent mailbox projection. |
| `mailbox.bind` | Installation-private; `mailbox_id`, `harness`, `external_session_id` | Binds a harness session to a mailbox. |
| `mailbox.context` | Installation-private; mailbox ID and repository context | Records one signed context snapshot for abandoned-session search. |
| `question` | Private, peer-addressed, or account-addressed; `body`, optional `details` | Starts a question thread. An account question projects into every active device's human mailbox. |
| `answer` | Private, peer-addressed, or account-addressed; `body`, optional `details` | Adds one answer to a question thread. An account answer directly names the source agent and also replicates account state. |
| `message` | Private, peer-addressed, or account-addressed; `body`, optional `details` | Starts an async message thread. |
| `thread.cancel` | Private, peer-addressed, or account-addressed; optional `reason` | Records cancellation without deleting answers. |
| `message.archive` | Installation-private or account-addressed; `target_event_id`, optional `reason` | Hides a message from open views. |
| `message.reject` | Installation-private or account-addressed; `target_event_id`, optional `reason` | Records rejection and archives the message. |
| `peer.trust` | Installation-private; peer installation ID, signer key ID, optional name and relay hints | Allows signed peer traffic. |
| `peer.distrust` | Installation-private; peer installation ID | Stops later peer projection. |
| `mailbox.share` | Installation-private; mailbox ID and peer installation ID | Lets one peer address one agent mailbox. |
| `mailbox.share.revoke` | Installation-private; mailbox ID and peer installation ID | Stops later direct delivery to that mailbox. |
| `human.account.create` | Installation-private; account ID, creator installation and key, signed label | Creates a human account and makes its creator the first active device. |
| `human.account.select` | Installation-private; account ID | Selects one active account as the local default. |
| `human.device.grant` | Account-addressed with a direct target; account, creator, target installation and key, signed label, relay hints | Records creator authority for one device. |
| `human.device.accept` | Account-addressed; the exact grant payload | Proves that the invited installation controls its root key and accepts the grant. |
| `human.device.revoke` | Account-addressed; the exact device identity | Removes current device authority without erasing history. |

Archive, reject, cancel, distrust, and share revoke events are signed tombstones. They change projections but never erase prior canonical bytes.

## Trust and mailbox access

Only the local root key may create installation-private control state. Peer trust is one-way. A local `peer.trust` event does not claim that the remote installation trusts the local installation.

A trusted remote installation may address the reserved local human mailbox. The remote installation may address an agent mailbox only when one of these rules holds:

- A current local `mailbox.share` event names that peer and mailbox.
- The remote event is an answer to a local question that directly addressed that remote sender mailbox.

Knowing an agent mailbox UUID grants no rights. A share revoke stops later projection but cannot erase data that the peer already received.

Trust and share changes use causal parents. The reducer finds maximal facts in the causal graph. Concurrent trust and distrust fail closed as distrusted. Concurrent share and revoke fail closed as revoked. A later trust or share must causally descend from the conflicting tombstone to become active.

Human account authority uses a separate causal graph. The account creator signs grants and revokes. The invited installation signs acceptance with the exact installation key, label, and relay hints from the grant. Membership needs both a grant and a causally later acceptance. A revoke that follows or races with the maximal acceptance makes the device inactive. A later accepted regrant can restore the device only when the new graph descends from the revoke. Missing creation or grant parents remain unresolved. Conflicting account creation events for one UUID do not create an account.

Peer trust and human account authority stay separate. A trusted peer is not an account device. An account device gets no direct access to an agent mailbox. The local default-account selection must be signed by the local root and must causally include that installation's account creation or accepted grant.

Every account action must causally include the current membership frontier for its signer. A receiver checks membership at that causal point, not only the receiver's latest device view. A valid event from before a later revoke stays valid. A revoked device cannot create a valid later account action. One canonical account event fans out through separate encrypted wrappers, but every device reduces the same canonical event ID.

## Reduction

The reducer assigns one status to each event:

- `projected`: valid, authorized, causally usable, and applied.
- `unresolved`: valid and authorized, but one or more required parents are absent or unusable.
- `unsupported`: the signature is valid, but the local binary does not support the Nostr kind, event type, or schema version.
- `invalid`: the signature, ID, JSON, known payload, size, identity, or causal thread rule is invalid.
- `unauthorized`: the signer, peer state, or mailbox route lacks authority.

An authentic event from a newer compatible schema stays byte-for-byte intact as `unsupported`. An upgraded reducer may register that schema and retry reduction. Invalid or unauthorized input never changes a domain projection.

Reduction must be idempotent and return the same state for every topological arrival order. Repeated wire forms with one Nostr event ID represent one canonical event. Implementations must not use receipt order or wall-clock order to choose semantic state.

The display order places parents before children. Among ready concurrent events, it sorts by signed `created_at` and then event ID. Display order has no semantic force.

## Answers and cancellation

A question may have several valid answers. `wait` returns the first locally available, not-yet-consumed answer in display order. The protocol does not name a globally accepted answer.

Answer and cancellation are independent facts. A thread can be both answered and cancelled. For each answer and cancellation pair, the reducer records one relation:

- The answer causally precedes the cancellation.
- The answer causally follows the cancellation.
- The two events are concurrent.

HQ must not infer why an answer followed or raced with cancellation.

`wait QUESTION_ID` requires the local question event and proof that the calling mailbox sent it. `poll` and `get` may expose a valid addressed answer while its parent is missing, but must mark the causal history incomplete. A later signed causal child from a peer also proves that the peer received its parent; HQ does not need a receipt for that fact.

## Retention and non-domain state

The first release keeps canonical events without automatic pruning. Projections are disposable caches and must rebuild from the event log.

These node facts are not canonical events and remain unsigned:

- Relay attempts and error text.
- Subscription cursors and relay acceptance records.
- Projection checkpoints.
- Delivery leases, sync locks, and retry timers.
- UI focus, drafts, and cache data.

Direct database edits can bypass the protocol, but supported HQ code must apply durable state only through valid signed events.
