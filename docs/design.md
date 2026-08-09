# HQ design

HQ's durable event format and causal reduction rules are defined in [events.md](events.md). Schema version 5 stores exact signed event bytes as the source of truth. Mailbox, message, thread, peer, and share tables are disposable projections. [nostr.md](nostr.md) defines encrypted relay transport.

## Installation identity

One HQ state directory has one stable installation UUID and one secp256k1 root key. `hq identity init` creates `hq.key` with mode `0600`. The sibling SQLite database never stores the secret key. `identity show` prints only the installation UUID and public key.

State paths use `$XDG_STATE_HOME/hq` or `~/.local/state/hq`. Relay settings may later use `$XDG_CONFIG_HOME/hq` or `~/.config/hq`; schema version 5 keeps peer trust and relay hints in signed events and local installation relay settings in SQLite.

`identity export` writes a mode-`0600` backup. The backup keeps the installation UUID and encrypts the root key as NIP-49 `ncryptsec` data. Passwords enter through hidden terminal input, never command arguments. `identity import` restores an identity only when no key exists. Running the same imported identity on two active hosts is unsupported.

`identity reset --yes` deletes the key, database, and SQLite side files. Schema version 4 also drops older schema data without migration. HQ remains in green-field development.

## Mailboxes and context

One signed `mailbox.create` event creates each opaque mailbox UUID. The installation-wide human mailbox uses the reserved UUID. Signed `mailbox.bind` events bind agent mailboxes to unique `(harness, external session ID)` pairs. Namespacing keeps equal Codex, Claude Code, and Pi IDs separate.

The resolver checks `--session`, then `HQ_SESSION`, then `CODEX_THREAD_ID`, `CLAUDE_CODE_SESSION_ID`, and `PI_SESSION_ID`. Manual values use the `custom` harness name. HQ rejects conflicting built-in IDs.

Signed `mailbox.context` events record directory, Git common directory, compact remote identity, worktree, and branch. Signed message payloads carry the same immutable context. Context aids display and abandoned-mailbox search; context does not grant access. Unsigned `last_seen_at` data records local process activity.

## Event-backed writes

Every supported durable domain write follows one path:

1. Build typed event content.
2. Sign canonical bytes with the installation root key.
3. Start one SQLite transaction.
4. Insert the exact signed bytes.
5. reduce the full canonical event set.
6. Rebuild the affected projections and derive peer-addressed outbox work.
7. Commit all rows together.

A reducer or projection error rolls back the event insert. Direct SQLite edits remain outside the supported state model.

The current reducer uses the full event set on each write. The simple path gives tests a clear result and suits the current low event count. A later incremental reducer may replace the full pass if the event count makes the cost visible.

User commands accept a signed message UUID from the payload. Causal parent links and transport deduplication use the canonical Nostr event ID. An answer points to the root question event and carries the original user message UUID as `reply_to` in the projection.

## SQLite tables

`canonical_events` retains exact signed bytes, identity fields, event type, scope, and the last reduction status. `causal_edges` indexes parent links. `projection_checkpoint` records the latest full rebuild.

`mailboxes`, `harness_bindings`, `mailbox_contexts`, `messages`, `threads`, `peers`, and `mailbox_shares` are rebuildable projections. `outbox` stores exact canonical bytes for peer-addressed work; later Nostr transport will add the exact signed outer wrapper before publish.

`delivery_facts`, `mailbox_activity`, and projection checkpoints are unsigned local facts. Delivery claims use a 30-second lease because SQLite and stdout cannot share a transaction. A crash after stdout and before completion can cause one retry.

`relays`, `outbound_relay_attempts`, `inbound_wrappers`, and `relay_sync_state` hold unsigned transport facts. `outbox` adds the exact signed gift wrap before publish. `inbound_staging` holds temporary receive failures for a later retry. `quarantine` holds permanent failures and never retries them on its own. Quarantine keeps the raw wrapper, relay, event ID, reason, and receive time. The local cap is 1,000 rows, 16 MiB, and 30 days; the oldest row leaves first.

CLI writes and the optional daemon open SQLite at the same time under WAL mode. A separate advisory sync lock grants one process relay-worker ownership without blocking local event commits. Foreground commands wake the daemon through a protected Unix socket when the daemon owns that lock. Polling repairs a lost wake. The socket does not carry message bodies or signing keys.

SQLite uses strict tables, foreign keys, WAL mode, `synchronous=FULL`, a five-second busy timeout, one connection per process, and mode `0600` for the database file. HQ does not tail SQLite's WAL and does not add a second file WAL.

## Trust and sharing

`peer add` signs an installation-private trust event. Trust is one-way. The remote installation must add its own trust event. `peer distrust` signs a tombstone and stops future peer projection.

`mailbox share` lets one trusted peer address one agent mailbox. `mailbox revoke` signs a tombstone that stops later projection rights; revocation cannot erase data that the peer already received. A trusted peer may address the human mailbox without a mailbox share.

The first release keeps peer policy local to signed canonical events. Signed invitation flows, key rotation, more than one active host per identity, and a separate signer process remain deferred.

## Trust boundary

Local same-user actors are cooperative. Mode `0600` stops other Unix users but does not stop an agent running as the same user from reading the key or database. Relay transport will use signatures to stop forged traffic from an unknown installation. A later privileged signer can narrow local key access.

## TUI context

The TUI refreshes every minute without clearing or retargeting an active draft. For the selected message, HQ loads the branch and shared Git remotes, then looks up an open pull request in the background. GitHub and GitLab remotes use compact labels. Context failures never block mailbox use.
