# HQ design

## Mailboxes and identity

HQ models communication as immutable messages between scoped mailboxes. A mailbox key has two parts:

1. `directory` is the absolute, clean work directory.
2. `session_id` identifies an agent run or the reserved `human` actor.

An agent session comes from `--session`, `HQ_SESSION`, or `CODEX_THREAD_ID`, in that order. Every message has a UUIDv7. The message primary key is `(directory, recipient_session, id)`, and `id` is also unique for direct CLI lookup.

No authentication or mailbox privacy exists yet. A caller that knows another session ID can query or poll that mailbox.

## Message flow

`ask` creates an agent-to-human message. A human reply performs one SQLite transaction:

1. Archive the inbound human-mailbox row.
2. Insert a new human-to-agent message with `reply_to` set to the inbound ID.

An unsolicited human message follows the same outbound path without `reply_to`. `wait` claims a reply to one message. `poll` claims any incomplete message for an agent mailbox. Successful delivery sets both `completed_at` and `archived_at`.

Sent and Archived are independent TUI filters. Sent adds rows where the sender is `human`. Archived adds archived rows in each visible direction. The open human inbox stays visible in all four filter states.

## Storage contract

`internal/store.Store` owns message creation, atomic replies, queries, archival, and leased delivery. SQLite is the first implementation. A future shared service can implement the same interface.

The SQLite database uses strict tables, foreign keys, WAL journaling, `synchronous=FULL`, a five-second busy timeout, one connection per process, state checks, and indexes for inbox, sent, replies, and delivery queries. The database file mode is `0600`.

Version 2 deliberately does not translate the old question state machine. HQ renames the old table to `legacy_questions_v1` and starts empty mailboxes.

## Delivery boundary

HQ leases a message before delivery, builds output in memory, writes stdout once, and then completes the row. SQLite and stdout cannot share one transaction. A process crash after the write and before completion can yield the same message twice. HQ favors retry over message loss.

## TUI context

The TUI refreshes every minute without clearing or retargeting an active draft. For the selected message directory, HQ loads the branch and shared git remotes, including linked-worktree settings, then looks up an open pull request in the background. GitHub and GitLab remotes use compact `name: owner/repo` labels. The `go-gh` client reads the same token sources as `gh`. Context failures never block mailbox use.
