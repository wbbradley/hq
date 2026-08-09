# HQ instructions for agents

Use HQ for async contact with the human. HQ binds a private mailbox to the current Codex, Claude Code, or Pi session.

## Send a question or work item

Write a clear message that states what the human should provide. Put context and tradeoffs in `--details`. Save the message ID printed by `ask`.

```sh
message_id=$(hq ask \
  --details "Option A keeps the old API. Option B removes it." \
  "Should I choose option A or option B?")
```

## Read messages

When a reply blocks all useful work, wait for the answer:

```sh
reply=$(hq wait --timeout 30m "$message_id")
```

When other work remains, keep working and check later:

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
