# HQ canonical event protocol

Status: clean-break protocol, canonical schema 3 only. Earlier canonical schemas are rejected and
must not be imported or translated into a schema-33 database.

HQ derives durable state from signed canonical events. SQLite tables, relay queues, and user views are indexes of those events. A supported HQ command must not change durable domain state without creating and applying a valid event.

The format follows [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md) for event IDs and Schnorr signatures. [nostr.md](nostr.md) defines the relay wrapper. Relay URLs and transport retry data are not part of a canonical event.

## Identity

Each installation has:

- A stable random UUID called the installation ID.
- One root secp256k1 key in the first release.
- Any number of mailbox UUIDs.

A logical human account has its own stable UUID and may grant several installation identities the right to act as devices for that human. The account UUID is not a mailbox address, installation ID, or public key.

The installation ID does not change when a key changes. Schema 3 does not implement key rotation,
but every event contains `signer_key_id`. The current key ID is the root public key as 32-byte
lowercase hex.

A full mailbox address is `(installation_id, mailbox_id)`. A bare mailbox UUID is not a network address and grants no access.

## Nostr envelope

HQ uses provisional regular Nostr kind `7281`. The project may register or change this kind before
HQ 1.0. The Nostr `tags` array is empty. All HQ fields live in `content` as compact JSON.

HQ computes the Nostr event ID from the NIP-01 serialization and signs that ID with BIP-340 Schnorr over secp256k1. A receiver must check the content-derived ID and signature before it parses or applies HQ content.

The full wire event may not exceed 65,536 bytes. HQ stores the exact received bytes for audit and retry, while the Nostr event ID remains the logical deduplication key.

## Common content

The Nostr `content` string contains this object in field order when HQ creates an event:

```json
{
  "schema": 3,
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
  "authorities": ["the same signed account membership event ID"],
  "scope": "account-addressed",
  "payload": {
    "body": "Which port should I use?"
  }
}
```

Fields have these rules:

- `schema` is exactly `3`. Other versions are retained only as unsupported input diagnostics and
  never project.
- `type` selects one payload schema and reducer rule.
- `installation_id` names the installation that created the event. A message sender must belong to that installation.
- `signer_key_id` must equal the Nostr event public key until key grants exist.
- `sender` is present on message and activity events. A direct message route has at most one
  `recipient`. An account question or activity omits `recipient` and names a human account in
  `audience`.
- `thread_id` is present on child thread events and absent on a root question or async message.
- `parents` is a set of causal event IDs in lexical order. It may contain at most 64 IDs and no duplicate.
- `authorities` is a typed role within that causal set: every authority ID must also occur in
  `parents`. Addressed actions name the exact capability or account-membership facts that authorize
  them; authority is never inferred from unrelated parents.
- `scope` is `installation-private`, `peer-addressed`, or `account-addressed`. Account-addressed events require a matching human-account audience. `public` is reserved and rejected in the first release.
- `origin` may name a source installation and event for an import or forward. An origin is not a causal parent.
- `payload` is a strict object selected by `type`.

Event bodies and details must contain valid UTF-8. A body may not exceed 32,768 bytes. Details may
not exceed 16,384 bytes. Schema 3 carries no files or binary data.

### Text payload

The schema-3 text payload contains `message_id`, `body`, `details`, `purpose`, `context`,
`actor_label`, and these typed fields:

- `presentation`: empty or one of `update`, `final-answer`, `status`, and `notice`;
- `correlation`: an opaque provider/session pair, with optional operation and operation-scoped item
  and request IDs; item or request identity requires an operation;
- `technical_sections`: ordered diagnostic/display sections with a lowercase namespaced machine
  name and ordered fields containing a lowercase machine key, optional printable display label,
  and UTF-8 string value.

Presentation and correlation are semantics: routing, conversation and action identity, reply
targeting, final-answer choice, and other behavior use dedicated typed fields. Technical sections
are inert disclosure metadata. Their namespaces identify provenance, keys identify fields, and
labels affect display only. Consumers must not inspect a technical namespace, key, label, or value
to make a domain decision. If a value becomes behavioral, it needs a dedicated typed field.

Provider values and correlation IDs are harness-neutral and opaque. Providers are at most 128
bytes; each correlation ID is at most 512 bytes. A message may carry at most 16 technical sections,
32 fields per section, and 128 fields total. Namespaces and keys are at most 128 bytes, labels 256
bytes, values 4,096 bytes, and the aggregate namespace/key/label/value content 16,384 bytes.
Namespace/key pairs may not repeat. All names and identities are validated, all text must be valid
UTF-8, and final signing still enforces the complete 65,536-byte wire-event limit after JSON
escaping and envelope overhead.

HQ-owned namespaces begin with `hq.`. Current message producers use `hq.harness.output`,
`hq.harness.status`, `hq.harness.request`, `hq.project.output_provenance`,
`hq.project.resource_health`, and `hq.project.pending_message`. Other producers may use their own stable namespace; readers
render unknown namespaces generically rather than maintaining an allowlist. Technical sections are
not an access-control or secret-storage mechanism and share the message's audience.

All text-message writers emit schema 3. No store query, RPC client, reducer, or UI parses `Details`
for structure or behavior.

Local message payloads also carry a stable message UUID and an immutable repository-context snapshot. `send` prints this UUID; it is the short user-facing handle accepted by `get`, `wait`, `answer`, and `cancel`. The Nostr event ID remains the signed deduplication key and causal reference. Remote protocol work may change the handle format before HQ 1.0.

### Harness activity payload

`harness.activity` is a schema-3 canonical event and a conversation entry, but it is not a message.
Its strict harness-neutral payload contains typed provider/session/operation correlation, an
optional operation-scoped item ID, activity kind and status, bounded title/body, explicit
truncation, occurrence time in Unix milliseconds, runtime-lifetime ID, and a positive provider
event sequence. The supported kinds are operation status, plan, diff, completed command, completed
file change, completed tool call, and progress. Provider method names, JSON-RPC envelopes, raw model
responses, reasoning, token deltas, and message technical sections are not activity payload data.

The event sender is the originating full agent mailbox address. The provider and correlation IDs
are opaque namespaces, not network identities; the reducer's coalescing key includes source
installation, source mailbox, provider, session, operation, kind, and item. An activity has no
recipient or HQ thread ID. It may be installation-private for a genuinely local conversation, or
account-addressed to the selected human account with current membership parents. It may never be
peer-addressed or public. The current harness writer uses the account audience and ordinary
per-active-device encrypted outbox fanout.

Titles are limited to 1 KiB, general and command bodies to 12 KiB, and progress bodies to 4 KiB.
Truncation preserves valid UTF-8 and sets `truncated`. The authoring path also measures the complete
escaped, signed envelope and may shorten the body further to stay below 65,536 bytes; the nominal
field bounds are not a substitute for the wire bound.

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
| `mailbox.bind` | Installation-private; `mailbox_id`, `harness`, `external_session_id` | Permanently records a harness session binding; named mailboxes may retain several. |
| `mailbox.context` | Installation-private; mailbox ID and repository context | Records one signed context snapshot for abandoned-session search. |
| `agent.name.claim` | Installation-private; `name`, `mailbox_id` | Permanently claims a local lowercase agent name for one agent mailbox. |
| `agent.retire` | Installation-private; `name`, `mailbox_id` | Retires a name and mailbox without permitting later reuse. |
| `agent.session.select` | Installation-private; name, mailbox, harness, external session ID, and exact repository context | Selects the named agent's current harness session while retaining rebuildable per-session directory history and selection times. |
| `agent.session.rename` | Installation-private; agent name, mailbox, harness, external session ID, and thread name | Sets or clears mutable display metadata for an existing bound session without selecting it or changing runtime state. |
| `question` | Private, peer-addressed, or account-addressed; schema-3 text payload | Starts a question thread. An account question projects into every active device's human mailbox. |
| `answer` | Private, peer-addressed, or account-addressed; schema-3 text payload | Adds one answer to a question thread. An account answer directly names the source agent and also replicates account state. |
| `message` | Private, peer-addressed, or account-addressed; schema-3 text payload | Starts an async message thread. |
| `harness.activity` | Installation-private or account-addressed; schema-3 typed activity payload | Adds non-actionable runtime telemetry to a provider/session conversation. |
| `thread.cancel` | Private, peer-addressed, or account-addressed; optional `reason` | Records cancellation without deleting answers. |
| `message.archive` | Installation-private or account-addressed; `target_event_id`, optional `reason` | Hides a message from open views. |
| `message.restore` | Installation-private or account-addressed; `target_event_id` | Causally supersedes an archive and returns the message to open views. |
| `message.reject` | Installation-private or account-addressed; `target_event_id`, optional `reason` | Records rejection and archives the message. |
| `peer.binding.set` | Installation-private; peer installation ID, signer key ID, optional name and relay hints | Binds a remote identity for local routing and signature checks. |
| `peer.binding.block` | Installation-private; peer installation ID | Stops new local transport while retaining authorized history. |
| `project.event` | Account-addressed; project ID, previous event ID, operation, canonical body | Replicates the home-issued linear project history to active human devices. |
| `project.command` | Account-addressed human-device control envelope | Queues one expected-head project mutation for its home installation. |
| `project.command.result` | Account-addressed human-device result envelope | Reports received, committed, or rejected command state and the current project head. |
| `mailbox.access.grant` | Peer-addressed; target mailbox and grantee installation/key | Creates a directional authority root for one mailbox. |
| `mailbox.access.revoke` | Peer-addressed; exact grant payload and grant authority | Removes access for concurrent and causally later actions. |
| `mailbox.access.observe` | Peer-addressed; grant and accepted-message IDs | Receiver-signed proof that an authorized action preceded revocation. |
| `human.account.create` | Installation-private; account ID, creator installation and key, signed label | Creates a human account and makes its creator the first active device. |
| `human.account.select` | Installation-private; account ID | Selects one active account as the local default. |
| `human.device.grant` | Account-addressed with a direct target; account, creator, target installation and key, signed label, relay hints | Records creator authority for one device. |
| `human.device.accept` | Account-addressed; the exact grant payload | Proves that the invited installation controls its root key and accepts the grant. |
| `human.device.revoke` | Account-addressed; the exact device identity | Removes current device authority without erasing history. |

Archive, reject, cancel, binding block, and capability revoke events are signed tombstones. They
change projections but never erase prior canonical bytes.

## Trust and mailbox access

Only the local root key may create installation-private control state. A peer binding is local route
and key metadata, not global authority. Binding is one-way; setting it does not claim that the
remote installation has reciprocated. Blocking prevents new transport without reclassifying valid
history.

Mailbox access is also directional. The mailbox owner signs `mailbox.access.grant` for one exact
grantee installation/key and one target mailbox. Every peer-addressed question, answer, or message
names exactly one matching grant in both `parents` and `authorities`. Knowing a mailbox UUID or
having a local peer binding grants no access.

Revocation names the grant as authority and causally descends from the grant and the owner's current
observation frontier. An action proven by `mailbox.access.observe` to precede the revoke remains
authorized. An action causally after or concurrent with the revoke fails closed. A later grant can
restore access because it is a new explicit authority root. Revocation must be deliverable before a
local route is blocked.

Human account authority uses a separate causal graph. The account creator signs grants and revokes. The invited installation signs acceptance with the exact installation key, label, and relay hints from the grant. Membership needs both a grant and a causally later acceptance. A revoke that follows or races with the maximal acceptance makes the device inactive. A later accepted regrant can restore the device only when the new graph descends from the revoke. Missing creation or grant parents remain unresolved. Conflicting account creation events for one UUID do not create an account.

Peer trust and human account authority stay separate. A trusted peer is not an account device. An account device gets no direct access to an agent mailbox. The local default-account selection must be signed by the local root and must causally include that installation's account creation or accepted grant.

Every account action must list its exact account-creation or accepted-device membership facts in
both `parents` and `authorities`. A receiver checks those authorities at that causal point, not only
the receiver's latest device view. Arbitrary causal parents never imply membership. A valid event
from before a later revoke stays valid. A revoked device cannot create a valid later account action.
One canonical account event fans out through separate encrypted wrappers, but every device reduces
the same canonical event ID.

Account-addressed activity follows this exact rule. An active source device can fan one canonical
activity event to all active human devices. A wrapper from a revoked source is decrypted, rejected
by causal membership authorization, and quarantined without changing messages, activity, or inbox
state. An unrelated account audience does not route to the local account, and peer/public activity
fails validation before projection.

## Reduction

The reducer assigns one status to each event:

- `projected`: valid, authorized, causally usable, and applied.
- `unresolved`: valid and authorized, but one or more required parents are absent or unusable.
- `unsupported`: the signature is valid, but the local binary does not support the Nostr kind,
  event type, or canonical schema. Schema 1 and 2 input is unsupported by design.
- `invalid`: the signature, ID, JSON, known payload, size, identity, or causal thread rule is invalid.
- `unauthorized`: the signer, peer state, or mailbox route lacks authority.

Unsupported input stays byte-for-byte intact for diagnostics but never changes a domain projection.
There is no compatibility decoder or schema translation path.

Reduction must be idempotent and return the same state for every topological arrival order. Repeated wire forms with one Nostr event ID represent one canonical event. Implementations must not use receipt order or wall-clock order to choose semantic state.

Conversation display order places parents before children. Among ready events it sorts by signed
`created_at`; activity from the same signed second additionally uses occurrence milliseconds,
source/runtime identity, provider sequence, and event ID as stable tie-breakers. Receipt time and
SQLite row order are never inputs. This order is presentation order, not causal authority.

Messages and activity share this reducer order but remain separate semantic streams. The typed
`conversation/entries` read derives and slices the causal order, using event IDs as stable identity.
Its message values retain typed schema-3 presentation, correlation, and technical sections. The
legacy `conversation/history` shape remains message-only. Conversation summaries, open/unread
counts, delivery, reply/archive targets, drafts, and final-answer selection are also message-only.

Activity projection is latest-wins per full source/provider/session/operation/kind/item key.
Operation, plan, and diff are snapshots; repeated item and progress keys coalesce; completed
command/file/tool events and terminal operation states remain durable logical history. Winner
selection uses canonical conversation order and source sequence, never receiver clocks.

## Answers and cancellation

A question may have several valid answers. `ask` and `wait` return the first locally available, not-yet-consumed answer in display order. The protocol does not name a globally accepted answer.

Answer and cancellation are independent facts. A thread can be both answered and cancelled. For each answer and cancellation pair, the reducer records one relation:

- The answer causally precedes the cancellation.
- The answer causally follows the cancellation.
- The two events are concurrent.

HQ must not infer why an answer followed or raced with cancellation.

The wait phase of `ask`, and `wait QUESTION_ID`, requires the local question event and proof that the calling mailbox sent it. `poll` and `get` may expose a valid addressed answer while its parent is missing, but must mark the causal history incomplete. A later signed causal child from a peer also proves that the peer received its parent; HQ does not need a receipt for that fact.

## Retention and non-domain state

The first release keeps canonical events without automatic pruning. Projections are disposable caches and must rebuild from the event log.

Canonical and projected activity retention intentionally differ. Superseded snapshots and progress
events remain exact signed canonical bytes. `harness_activities` contains only reducer-selected
winners and retains the newest 200 projected progress keys per source mailbox/provider session. A
full rebuild deterministically reapplies that cap. The legacy activity-list read returns at most the
newest 1,000 projected rows in chronological order; unified conversation pages use the normal
200-entry page cap and expose the same disposable projection, not every superseded canonical event.

Schema 33 keeps no dense display-order column. Mixed conversation history rebuilds one canonical
order from signed times, causal parents, typed activity correlation, and stable event-ID
tie-breakers.

These node facts are not canonical events and remain unsigned:

- Relay attempts and error text.
- Subscription cursors and relay acceptance records.
- Projection generation metadata and dependency indexes.
- Delivery leases, sync locks, and retry timers.
- UI focus, drafts, and cache data.
- Codex worker processes, caller environments, paths under validation, request receipts, runtime phases, presence, and ownership leases.

Direct database edits can bypass the protocol, but supported HQ code must apply durable state only through valid signed events.
