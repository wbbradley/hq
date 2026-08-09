# HQ design

## Mailboxes and harness bindings

HQ gives each mailbox an opaque UUID. One installation-wide mailbox represents the human. Each agent mailbox has one unique binding made from a harness name and the harness's external session ID. Namespacing prevents equal Codex, Claude Code, and Pi IDs from sharing a mailbox.

The resolver checks an explicit `--session` value, then `HQ_SESSION`, then the built-in harness variables. Manual values use the `custom` harness name. Built-in variables are `CODEX_THREAD_ID`, `CLAUDE_CODE_SESSION_ID`, and `PI_SESSION_ID`. HQ rejects a built-in conflict instead of guessing which harness owns the shell.

HQ does not show full external harness session IDs in standard CLI or TUI output. Agent labels contain the harness name and the last eight mailbox ID chars, such as `codex:6b906b1a`.

## Repository context and continuity

Directory, Git common directory, compact remote identity, worktree root, and branch are context, not mailbox keys. HQ writes an immutable context snapshot on each message and keeps a context history for each agent mailbox. Each successful mailbox resolve updates `last_seen_at`.

A resumed harness session resolves to the same mailbox after a process exit, host restart, or directory change as long as the same HQ database and external harness session ID remain available. HQ never deletes or reassigns a mailbox when a harness process vanishes.

`hq mailboxes` searches context history and reports likely mailbox matches. Context matches are only hints. A new harness session gets a new mailbox and cannot claim a match. A later handoff feature must make any mailbox transfer clear. Future process ownership may add host, boot, PID, process start, heartbeat, stale, zombie, and orphan state without changing mailbox IDs.

## Message flow

`ask` creates an agent-to-human message. A human reply performs one SQLite transaction:

1. Archive the inbound human-mailbox row.
2. Insert a new human-to-agent message with `reply_to` set to the inbound ID.

An unsolicited human message follows the same outbound path without `reply_to`. `wait` first proves that the resolved agent mailbox sent the given message, then claims a reply addressed to that mailbox. `poll` claims any incomplete message for the resolved mailbox across all directories. `get` keeps direct-ID read access for explicit cooperative inspection. Successful delivery sets both `completed_at` and `archived_at`.

`hq list` filters out archived messages unless the caller uses `--archived` or `--all`. Bare non-TTY `hq` narrows the open human inbox to the current directory. Sent and Archived remain independent TUI filters.

## Storage contract

`internal/store.Store` owns mailbox resolution, context search, message creation, atomic replies, queries, archival, and leased delivery. SQLite is the first implementation. A future relay-backed service can implement the same interface.

The SQLite database uses strict tables, foreign keys, WAL journaling, `synchronous=FULL`, a five-second busy timeout, one connection per process, state checks, and indexes for inbox, sent, replies, delivery, and context search. The database file mode is `0600`.

Schema version 3 drops all version 1 and version 2 HQ tables and rows. The project is still in green-field development, so HQ does not preserve or migrate old data.

## Trust boundary

Local actors are cooperative. Mailbox identity is not cryptographic, and direct database access can bypass CLI routing checks. Future work may bind installation and mailbox keys to signed grants, add relay routing and encrypted Nostr events, and isolate keys from a hostile local agent.

## Delivery boundary

HQ leases a message before delivery, builds output in memory, writes stdout once, and then completes the row. SQLite and stdout cannot share one transaction. A process crash after the write and before completion can yield the same message twice. HQ favors retry over message loss.

## TUI context

The TUI refreshes every minute without clearing or retargeting an active draft. For the selected message context, HQ loads the branch and shared Git remotes, including linked-worktree settings, then looks up an open pull request in the background. GitHub and GitLab remotes use compact `name: owner/repo` labels. Context failures never block mailbox use.
