# HQ design

HQ is a local-node application with signed, event-backed state. One node process owns an
installation's root key, SQLite database, reducer, projections, delivery leases, relay engine, and
subscription revisions. CLI and TUI processes are domain clients; they never open SQLite, sign
events, or own Codex workers themselves. The node supervises every HQ-managed Codex bridge and
its app-server child in process.

[events.md](events.md) defines canonical event schema 1 and causal reduction.
[nostr.md](nostr.md) defines encrypted remote transport. SQLite schema 11 stores the exact signed
event bytes as the source of truth and rebuildable projections derived from them.

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
deletes the key, database, and SQLite side files. HQ remains pre-1.0 and unsupported schema versions
may be reset rather than migrated; schema 7 migrates through durable mutation receipts (8), change
revisions (9), named-agent projections (10), and per-session history (11).

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
3. Insert the exact signed bytes and causal edges in one SQLite transaction.
4. Reduce the full canonical event set and rebuild projections.
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

## Revisions and subscriptions

`change_revision` contains one monotonic revision allocated in the same transaction as each
relevant change. Invalidations contain only the revision, broad topics (`messages`, `mailboxes`,
`network`, `peers`, `human`, `relays`, and `agents`), and an optional full-snapshot flag. They never contain
message bodies, database rows, signing material, or mutation results.

A subscription is registered before its acknowledged revision is read. Activation occurs only
after the acknowledgement response is written; queued changes at or below that revision are
discarded and every newer coalesced revision wakes the client. Per-subscriber queues have size one,
socket writes happen outside the commit path, and disconnect removes the subscription.

The TUI and Codex bridge subscribe before their initial snapshots. CLI `ask` and `wait` do the same.
All reload authoritative state on invalidation or reconnect and keep a five-minute periodic repair
fallback. TUI drafts, focus, and selection survive reloads.

## SQLite data

`canonical_events` retains exact signed bytes, identity fields, event type, scope, and reduction
status. `causal_edges` indexes parents and `projection_checkpoint` records rebuild progress.
`mailboxes`, historical bindings, named agents, selected sessions, contexts, messages, threads,
peers, shares, human accounts, and devices are rebuildable projections.

`outbox` contains one row per canonical event and recipient installation, including exact canonical
bytes, recipient key and relay hints, and the exact signed gift wrap before first publish.
`outbound_relay_attempts`, `inbound_wrappers`, `relay_sync_state`, staging, and quarantine are
unsigned transport facts. Mutation receipts and change revisions provide local RPC recovery and
subscriptions.

Delivery claims, mailbox activity, and named-agent ownership are unsigned local node facts. A named
agent lease persists across node restarts, expires naturally after crashes, and is never published
as a relay heartbeat. A suspended process may revive its expired lease only while its exact owner
token remains stored; any intervening acquisition replaces the token and defeats the stale renewal.
Claims use a 30-second lease because
stdout or a Codex app-server call cannot share the SQLite transaction. The Codex sidecar ledger and
deterministic app-server IDs reconcile the remaining crash window.

## Codex control plane and data plane

`hq codex` and the TUI capture their process environment and launch directory, ensure the node is
running, and issue the same local runtime RPC. The node-owned supervisor validates directories,
hosts one worker per durable agent, starts only `codex app-server --stdio`, waits for an exact
`thread/start` or `thread/resume` acknowledgement, then commits the selected session. Stable request
IDs make a lost local response safe to retry, while the named-agent lease rejects any independent
legacy owner. One shared thread-keyed ledger supports concurrent agents without sidecar races.

The caller environment exists only long enough to construct the child environment. HQ does not add
its names or values to canonical events, projections, mutation receipts, Nostr, the ledger,
HQ-authored log attributes, diagnostics, status, or RPC results. The protected diagnostic log does
capture app-server stderr verbatim, so child-emitted text is inside the local logging trust boundary.
Process ownership, paths, presence, and runtime phases are also installation-local
and never enter Nostr. Durable agent names, thread bindings, selections, mutable thread names, and
per-session repository context are signed installation-private facts. A thread-name change is
separate from selection and runtime state: it updates the session projection without starting,
stopping, or switching a worker. Presentation code resolves names through the agent-session domain
abstraction instead of copying mutable labels into immutable messages or runtime records. Mailbox
questions, answers, messages, and relay delivery are the Nostr data plane.

Node stop or restart cancels every worker and leaves its durable selection intact and offline;
workers are not automatically restarted. A future remote controller must address the owning node's
control plane, where paths are interpreted and validated.

The node configures strict tables, foreign keys, WAL mode, `synchronous=FULL`, a five-second busy
timeout, one database connection, and mode `0600`. WAL is an SQLite durability mechanism inside the
single owning process, not a client notification or multiprocess coordination channel.

## Human accounts, peers, and mailboxes

A human account UUID is separate from an installation and its reserved human mailbox. A creator
grants a device; the target must causally accept the grant before becoming active. Creator revokes
win until a later grant and acceptance descend from them. Pairing bundles carry exact signed account
authority history so a new device can verify earlier devices before relay catch-up.

Account-addressed events fan one canonical fact out to every active device. Agent mailbox bindings
and delivery leases remain installation-local. Peer trust is one-way and binds installation UUID,
root public key, label, and relay hints. A trusted peer can address the human mailbox; an agent
mailbox additionally requires an active signed share. Revocation stops later projection but cannot
erase already received data.

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
