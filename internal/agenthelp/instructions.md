# HQ instructions for agents

Use HQ to ask the human questions and send asynchronous messages. HQ binds a private mailbox to the current Codex, Claude Code, or Pi session.

## Ask a question

Write a clear question that states what the human should provide. Put context and tradeoffs in `--details`. `ask` waits until the human replies and prints the reply.

```sh
reply=$(hq ask \
  --details "Option A keeps the old API. Option B removes it." \
  "Should I choose option A or option B?")
```

Do not add a timeout. The human may reply much later. Use `--timeout` only when the human has given a real deadline for the answer.

## Send without waiting

Use `send` only when the message is fire-and-forget or useful work can continue before the reply. It prints the saved message ID.

```sh
message_id=$(hq send "I finished the migration. Review it when convenient.")
```

If a later reply becomes blocking, wait indefinitely for that specific message:

```sh
reply=$(hq wait "$message_id")
```

Use `poll` for unsolicited messages and replies that were intentionally left asynchronous:

```sh
hq poll
```

`poll` reads replies and new messages for the current agent session. An empty mailbox exits with code 3 and prints nothing.

Let HQ detect the session. Do not set or save a session ID in normal use. If HQ reports a missing or unclear session, report the error.

If HQ reports a missing installation identity, tell the human to run `hq identity init`. Agents must not manage installation keys or transport settings.

## More detail

- `hq agents commands`: syntax, output, and exit codes
- `hq agents sync-semantics`: local saves, relay sync, and offline use
- `hq agents delivery-semantics`: leases, duplicate reads, threads, and cancellation
