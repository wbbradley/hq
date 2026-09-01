# HQ design

HQ is a local-node application with signed, event-backed state. One node process owns an
installation's root key, SQLite database, reducer, projections, delivery leases, relay engine, and
subscription revisions. CLI and TUI processes are domain clients; they never open SQLite, sign
events, or own Codex workers themselves. The node supervises every HQ-managed Codex bridge and
its app-server child in process.

[events.md](events.md) defines canonical schema 3 and causal reduction.
[nostr.md](nostr.md) defines encrypted remote transport. SQLite schema 33 stores the exact signed
event bytes as the source of truth and rebuildable projections derived from them.
[projects.md](projects.md) defines the project, resource-claim, assignment, mailbox, and
remote-control model implemented by the daemon, RPC clients, CLI, and TUI.

## Identity, state, and runtime paths

One state directory contains one stable installation UUID, one secp256k1 root key, and one sibling
SQLite database. `hq identity init` creates `hq.key` with mode `0600`; the secret key is never stored
in SQLite. `hq identity show` prints only the installation UUID and public key. Export encrypts the
secret as NIP-49 `ncryptsec`; import refuses to overwrite an existing identity. Running one imported
identity on two active hosts is unsupported.

The default state directory is `$XDG_STATE_HOME/hq` or `~/.local/state/hq`. `HQ_DB` and global
`--db` select another database. Runtime socket and instance metadata paths are derived from the
canonical database path. Short paths use a sibling socket; long Unix paths use a stable hashed name
under `$XDG_RUNTIME_DIR/hq` or the platform runtime fallback. Socket and metadata permissions are
restricted to the current user.

The node owns a single append-only structured diagnostic sink at `~/logs/hq.log`. It uses the Go
standard library's `log/slog` text handler at debug level; HQ creates `~/logs` with mode `0700` when
absent and always protects the log with mode `0600`. Daemon, supervisor, bridge, and subprocess records share this sink and carry stable
correlation attributes rather than embedding context in prose.

`identity reset --yes` stops being safe once a node is using that identity: stop the node first. It
deletes the key, database, and SQLite side files. HQ remains pre-1.0; a non-empty database whose
schema is not 33 must be archived or removed rather than migrated in place.

## The local node

Every normal command connects to the local node. If no compatible owner is ready, the client
coordinates one detached auto-start and waits for a bounded readiness handshake. Concurrent starts
converge on the same advisory owner lock. A service manager is recommended for always-on relay
subscriptions, but callers do not need to start the node by hand.

The protected local socket multiplexes two versioned modes:

- Lifecycle RPC reports status and handles wake, stop, and restart. Restart closes existing
  sessions, creates a new instance ID, and refreshes runtime metadata.
- Domain RPC carries mailbox, message, peer, human-account, relay, lease, synchronization,
  change-subscription, session-history, and local Codex-runtime requests and responses.

Each handshake negotiates an explicit supported version range and exchanges build metadata. Equal
wire ranges with different builds are allowed and surfaced as an actionable restart notice. A
range mismatch is a typed incompatibility error that identifies whether the client or node is
stale. On Unix, the node uses a mode-`0600` local socket. Windows local transport remains explicitly
unsupported until named-pipe support exists.

Clients reconnect with bounded exponential backoff, renegotiate, resubscribe, and request an
authoritative full snapshot signal before reporting ready. An in-flight mutation keeps the same
mutation UUID and exact request object across a transport retry.

## Event-backed writes and mutation reconciliation

Every durable canonical write follows one transaction path:

1. Validate a typed domain request and its stable mutation UUID.
2. Build typed event content and sign canonical bytes in the node.
3. Insert the exact signed bytes and dependency indexes in one SQLite transaction.
4. Compute the dependency-closed affected facts with the pure reducer and patch their projections.
5. Derive durable per-recipient outbox rows.
6. Store the successful mutation result receipt and increment the change revision.
7. Commit, then publish a lightweight topic/revision invalidation.

A reducer, projection, receipt, outbox, or revision error rolls the transaction back. The mutation
receipt binds UUID, method, canonical request digest, encoded result, and commit time. Repeating the
same request after a lost response or node restart returns the committed result; reusing its UUID
for different input is rejected. Reply-plus-archive and lease completion remain atomic.

Locally signed events and remotely decrypted/verified events share the same canonical append,
reduction, projection, outbox, commit, and post-commit observer. Direct SQLite edits are outside the
supported state model.

### Typed message semantics and diagnostics

New question, answer, and message events use schema 3. Their presentation kind and harness-neutral
provider/session/operation/item/request correlation are dedicated semantic fields. Conversation
identity, action grouping, reply/archive targeting, final-answer selection, routing,
authorization, and ordering may use only typed domain state—not body, `Details`, or generic
technical metadata. `Details` is always supplementary human text.

Namespaced technical sections are ordered diagnostic/display data. Namespaces establish
provenance, keys establish field identity, and optional labels affect rendering only. The TUI hides
or shows whole sections with `i` and renders unknown namespaces generically; it does not maintain a
producer allowlist. Built-in message/event/thread/installation identifiers and repository/source
context are derived technical presentation groups governed by the same disclosure control. Mutable
thread names are resolved at display time from the typed provider/session pair and never copied
into immutable message events.

Canonical schema 3 has no legacy structural-line adapter. Current event, model, SQLite, domain RPC,
and local client representations round-trip typed fields and ordered technical-section JSON
directly. A full projection rebuild derives the same representation from the canonical log.

### Dual-stream conversations

A provider-namespaced harness session is one conversation with two ordered semantic streams.
Messages carry inbox, open/unread, delivery, reply, archive, draft, action-unit, and final-answer
behavior. Schema-2 `harness.activity` carries non-actionable operation status, plan, diff, completed
command/file/tool, and progress telemetry. Both use typed provider/session/operation/item
correlation and one reducer conversation order; neither reconstructs identity or presentation from
body or `Details`.

The `conversation/entries` domain read is a discriminated message/activity union. Every entry has a
canonical event ID and a position in the reducer's causal conversation order, and exactly one full
typed message or activity. Pages are sliced from that derived order with the event ID as stable
identity; no dense global rank is persisted. Messages hydrate their public UUID and typed semantics
for actions; activity has no action ID. The legacy `conversation/history` API remains message-only
and conversation summaries continue to derive only from messages.

Canonical source identity and correlation are different axes. Activity is associated with its full
originating installation/mailbox address. Provider/session/operation/item IDs are opaque adapter
correlation. Projection and query keys include source mailbox plus provider namespace so collisions
do not merge. Device-local message address labels, recipient installation presentation, and
delivery state may differ between devices even when canonical event IDs, typed semantics, and
mixed reducer order converge.

## Revisions and subscriptions

`change_revision` contains one monotonic revision allocated in the same transaction as each
relevant change. Invalidations contain only the revision, broad topics (`messages`, `mailboxes`,
`network`, `peers`, `human`, `relays`, and `agents`), and an optional full-snapshot flag. They never contain
message bodies, database rows, signing material, or mutation results.

A subscription is registered before its acknowledged revision is read. Activation occurs only
after the acknowledgement response is written; queued changes at or below that revision are
discarded and every newer coalesced revision wakes the client. Per-subscriber queues have size one,
socket writes happen outside the commit path, and disconnect removes the subscription.

The synchronous store actor publishes each committed revision by replacing one latest-value wake;
publication neither waits for Tokio nor allocates per subscriber. The daemon's sole local-session
pump awaits that observer directly alongside listener and session readiness, publishes the greatest
revision into the shared subscription hub, and performs one bounded nonblocking socket-delivery
pass. There is no timer between a normal store commit and daemon invalidation delivery.

The TUI and Codex bridge subscribe before their initial snapshots. CLI `ask` and `wait` do the same.
All reload authoritative state on invalidation or reconnect and keep a five-minute periodic repair
fallback. TUI drafts, focus, and selection survive reloads.

## SQLite data

`canonical_events` retains exact signed bytes, identity fields, event type, scope, and reduction
status. Schema 33 indexes causal and authority dependencies, unresolved waiters, event resources,
aggregate frontiers, projection support, layered reduction decisions, and reducer generation
metadata. Those indexes select a dependency-closed affected set for ordinary ingestion; an explicit
repair reduces the complete log as the offline oracle.

`mailboxes`, historical bindings, named agents, selected sessions, contexts, messages, harness
activity, threads, peers, mailbox capabilities, human accounts, and devices are rebuildable
projections. `messages` stores typed presentation/correlation and ordered technical-section JSON.
`harness_activities` stores canonical source/audience identity, typed correlation,
runtime/sequence, and truncation. Conversation order is derived causally by the reducer and is not
stored as a dense table column. Projection rows and dependency indexes are caches, never alternate
sources of canonical truth.

`outbox` contains one row per canonical event and recipient installation, including exact canonical
bytes, recipient key and relay hints, and the exact signed gift wrap before first publish.
`outbound_relay_attempts`, `inbound_wrappers`, `relay_sync_state`, staging, and quarantine are
unsigned transport facts. Mutation receipts and change revisions provide local RPC recovery and
subscriptions.

Delivery claims and named-agent ownership are unsigned local node facts. A named
agent lease persists across node restarts, expires naturally after crashes, and is never published
as a relay heartbeat. A suspended process may revive its expired lease only while its exact owner
token remains stored; any intervening acquisition replaces the token and defeats the stale renewal.
Claims use a 30-second lease because
stdout or a harness submission cannot share the SQLite transaction. The delivery ledger and stable
submission IDs reconcile the remaining crash window.

## Harness control plane and data plane

`hq harness --provider ID` and the TUI capture their process environment and launch directory, ensure
the node is running, and issue the same local runtime RPC. The node-owned supervisor validates
directories, hosts one logical instance per durable agent, and invokes the selected registered
factory. The Codex adapter starts `codex app-server --stdio` and translates exact `thread/start` or
`thread/resume` acknowledgements into neutral session readiness before selection. Stable request
IDs make a lost local response safe to retry, while the named-agent lease rejects any independent
legacy owner. One shared thread-keyed ledger supports concurrent agents without sidecar races.

The caller environment is sensitive local control-plane state. After a successful launch, the
supervisor retains one last-known-good launch template per named agent in daemon memory for automatic
offline wakeups. It wipes replaced templates and all templates on shutdown. HQ does not add
environment names or values to canonical events, projections, mutation receipts, Nostr, the ledger,
HQ-authored log attributes, diagnostics, status, or RPC results. The protected diagnostic log does
capture app-server stderr verbatim, so child-emitted text is inside the local logging trust boundary.
Process ownership, paths, presence, and runtime phases are also installation-local
and never enter Nostr. Durable agent names, thread bindings, selections, mutable thread names, and
per-session repository context are signed installation-private facts. A thread-name change is
separate from selection and runtime state: it updates the session projection without starting,
stopping, or switching a worker. Presentation code resolves names through the agent-session domain
abstraction instead of copying mutable labels into immutable messages or runtime records. Mailbox
questions, answers, messages, canonical harness activity, and relay delivery are the Nostr data
plane.

A committed local human message or answer addressed to an offline named Codex mailbox asks the
supervisor to resume the selected thread asynchronously. Concurrent messages coalesce into one wake.
Within one daemon lifetime the exact last-known-good environment, cwd, repository context, and yolo
setting are reused; after restart, the selected session context is combined with the sender's current
environment and launch defaults loaded by the daemon when it constructs the request. Initial prompts
are never replayed. Mutation-receipt replays repeat the idempotent wake attempt so a crash between
message commit and dispatch does not permanently strand queued input.

Supported provider events enter one bounded serialized persistence buffer. Assistant output,
terminal operation state, and completed command/file/tool records backpressure until accepted.
Running/plan/diff/progress snapshots can replace the same pending logical key at the buffer tail;
new keys also backpressure at capacity. One relay timeline is assigned before buffering and an
indivisible work item persists output before activity. Stable output IDs and the delivery ledger
reconcile a partial output-then-activity write. Normal provider shutdown closes intake and drains
all accepted durable/latest coalesced work using a relay-owned persistence context; a drain timeout
is surfaced as failure.

Node stop or restart cancels every worker and leaves its durable selection intact and offline;
workers restart on demand when a local human message arrives. A future remote controller must address
the owning node's control plane, where paths are interpreted and validated.

The node configures strict tables, foreign keys, WAL mode, `synchronous=FULL`, a five-second busy
timeout, one database connection, and mode `0600`. WAL is an SQLite durability mechanism inside the
single owning process, not a client notification or multiprocess coordination channel.

## Human accounts, peers, and mailboxes

A human account UUID is separate from an installation and its reserved human mailbox. A creator
grants a device; the target must causally accept the grant before becoming active. Creator revokes
win until a later grant and acceptance descend from them. Pairing bundles carry exact signed account
authority history so a new device can verify earlier devices before relay catch-up.

Account-addressed events fan one canonical fact out to every active device. Agent mailbox bindings
and delivery leases remain installation-local. A one-way peer binding records installation UUID,
root public key, label, and relay hints for local routing. Mailbox authority comes only from a
mailbox-owner-signed capability for one grantee installation/key and target mailbox. Peer actions
cite that grant explicitly; receiver observations retain pre-revocation history while concurrent
and later actions fail closed. Local blocking stops new transport without hiding authorized history.

Harness activity uses the same account fanout. The writer includes the active membership frontier,
creates one outbox row and encrypted wrapper per other active device, and receivers decrypt then
re-run causal membership authorization. Revoked-source activity is quarantined and changes no
projection. Activity cannot be public or peer-addressed; the protocol permits installation-private
activity only for genuinely local state.

Mailbox IDs are opaque UUIDs. Signed bindings namespace external IDs by harness (`codex`,
`claude-code`, `pi`, or `custom`). Signed repository context aids display and abandoned-mailbox
search but grants no access.

## Trust boundary and recovery

Local same-user actors are cooperative. File and socket mode `0600` excludes other Unix users but
does not stop another process running as the same user from reading the database or key. The node
centralizes access; it is not a privilege boundary. A future privileged signer could narrow this
trust boundary.

Remote events require valid signatures, encryption, identity binding, trust/account authority,
mailbox routing rights, schema support, and causal validity. Durable canonical IDs, mutation
receipts, exact gift-wrap reuse, wrapper/logical deduplication, and retained-relay catch-up make
restarts and retries idempotent. Quarantine is bounded to 1,000 rows, 16 MiB, and 30 days; transient
receive failures enter staging.

Canonical retention is broader than presentation retention. Exact activity events—including
superseded snapshots and progress that later falls out—remain in `canonical_events`. The disposable
activity projection coalesces logical keys and keeps only the newest 200 progress rows per source
mailbox/provider session; rebuild deterministically reapplies those rules. Schema 33 accepts only
fresh or already-current databases and derives mixed conversation order from canonical causality;
it does not retain a global display-order column.
