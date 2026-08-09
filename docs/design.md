# HQ design

HQ's durable event format and causal reduction rules are defined in [events.md](events.md). Schema version 7 stores exact signed event bytes as the source of truth. Mailbox, message, thread, peer, share, human-account, and device tables are disposable projections. [nostr.md](nostr.md) defines encrypted relay transport.

## Installation identity

One HQ state directory has one stable installation UUID and one secp256k1 root key. `hq identity init` creates `hq.key` with mode `0600`. The sibling SQLite database never stores the secret key. `identity show` prints only the installation UUID and public key.

State paths use `$XDG_STATE_HOME/hq` or `~/.local/state/hq`. Relay settings may later use `$XDG_CONFIG_HOME/hq` or `~/.config/hq`; schema version 7 keeps peer trust and relay hints in signed events and local installation relay settings in SQLite.

`identity export` writes a mode-`0600` backup. The backup keeps the installation UUID and encrypts the root key as NIP-49 `ncryptsec` data. Passwords enter through hidden terminal input, never command arguments. `identity import` restores an identity only when no key exists. Running the same imported identity on two active hosts is unsupported.

`identity reset --yes` deletes the key, database, and SQLite side files. Schema version 7 also drops older schema data without migration. HQ remains in green-field development.

## Human accounts and devices

A human account is a stable UUID separate from an installation and the reserved human mailbox. A fresh installation signs one account creation event and selects that account as its default. The account creator's installation is the first active device.

The creator signs a grant for each added installation. The grant binds the account, installation UUID, root public key, display label, and up to three relay hints. The added installation must sign an acceptance that causally names the grant before the device becomes active. A creator-signed revoke makes a later or concurrent membership state inactive. A later grant and acceptance must causally descend from the revoke to restore membership.

The pairing file contains the full signed account authority history, exact account-creation and target-grant bytes, and clear routing fields. `human join` checks every clear field against the signed events and checks the local installation UUID and key before one transaction imports the facts, adds minimum peer trust, signs acceptance, and changes the local default account. The history lets a new device verify events from every prior device. Repeating invite, join, acceptance import, or revoke does not add a second logical fact.

An agent question names the human account as its audience. Every active device projects the same canonical question into its local reserved human mailbox. An answer directly names the source agent address and keeps the account audience, so the source installation can deliver the reply while all account devices reduce the same thread and archive facts. Agent mailbox bindings and delivery leases remain local.

Account authority grants the device the right to act for the shared human account. Account authority does not expose any agent mailbox. Ordinary peer trust also does not grant human-account membership. The account creator alone can grant or revoke devices in this release. Protect and back up the creator identity; admin transfer and creator-key rotation are not yet supported.

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
6. Rebuild the affected projections and derive peer or per-device account outbox work.
7. Commit all rows together.

A reducer or projection error rolls back the event insert. Direct SQLite edits remain outside the supported state model.

The current reducer uses the full event set on each write. The simple path gives tests a clear result and suits the current low event count. A later incremental reducer may replace the full pass if the event count makes the cost visible.

User commands accept a signed message UUID from the payload. Causal parent links and transport deduplication use the canonical Nostr event ID. An answer points to the root question event and carries the original user message UUID as `reply_to` in the projection.

## SQLite tables

`canonical_events` retains exact signed bytes, identity fields, event type, scope, and the last reduction status. `causal_edges` indexes parent links. `projection_checkpoint` records the latest full rebuild.

`mailboxes`, `harness_bindings`, `mailbox_contexts`, `messages`, `threads`, `peers`, `mailbox_shares`, `human_accounts`, `human_account_devices`, and `human_account_default` are rebuildable projections. `outbox` has one row per `(canonical event, recipient installation)` and stores exact canonical bytes, recipient key and relay hints, and one exact signed outer wrapper before publish.

`delivery_facts`, `mailbox_activity`, and projection checkpoints are unsigned local facts. Delivery claims use a 30-second lease because SQLite and stdout cannot share a transaction. A crash after stdout and before completion can cause one retry.

`relays`, `outbound_relay_attempts`, `inbound_wrappers`, and `relay_sync_state` hold unsigned transport facts. `outbox` adds the exact signed gift wrap before publish. `inbound_staging` holds temporary receive failures for a later retry. `quarantine` holds permanent failures and never retries them on its own. Quarantine keeps the raw wrapper, relay, event ID, reason, and receive time. The local cap is 1,000 rows, 16 MiB, and 30 days; the oldest row leaves first.

CLI writes and the optional daemon open SQLite at the same time under WAL mode. A separate advisory sync lock grants one process relay-worker ownership without blocking local event commits. Foreground commands wake the daemon through a protected Unix socket when the daemon owns that lock. Polling repairs a lost wake. The socket does not carry message bodies or signing keys.

SQLite uses strict tables, foreign keys, WAL mode, `synchronous=FULL`, a five-second busy timeout, one connection per process, and mode `0600` for the database file. HQ does not tail SQLite's WAL and does not add a second file WAL.

## Trust and sharing

`peer add` signs an installation-private trust event. Trust is one-way. The remote installation must add its own trust event. `peer distrust` signs a tombstone and stops future peer projection.

`mailbox share` lets one trusted peer address one agent mailbox. `mailbox revoke` signs a tombstone that stops later projection rights; revocation cannot erase data that the peer already received. A trusted peer may address the human mailbox without a mailbox share.

The first release keeps peer policy local to signed canonical events. Human account invitations add only the peer trust needed for grant and acceptance transport. Key rotation, account admin transfer, more than one active host per installation identity, and a separate signer process remain deferred.

## Trust boundary

Local same-user actors are cooperative. Mode `0600` stops other Unix users but does not stop an agent running as the same user from reading the key or database. Relay transport will use signatures to stop forged traffic from an unknown installation. A later privileged signer can narrow local key access.

## TUI context

The TUI refreshes every minute without clearing or retargeting an active draft. For the selected message, HQ loads the branch and shared Git remotes, then looks up an open pull request in the background. GitHub and GitLab remotes use compact labels. Context failures never block mailbox use.
