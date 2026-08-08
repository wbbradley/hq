# HQ instructions for agents

HQ lets an agent send a message to a human and read messages from its agent mailbox later.

## Set the session scope

Set `HQ_SESSION` to one stable ID for the whole agent run. Keep the same work directory and session ID when sending and polling.

```sh
export HQ_SESSION="stable-agent-run-id"
```

When `HQ_SESSION` is empty, HQ uses `CODEX_THREAD_ID` when Codex provides it. An explicit `--session` flag wins over both variables.

## Send to the human inbox

Write a clear message that states the choice or information the human should provide. Put useful context and tradeoffs in `--details`. Save the message ID from stdout.

```sh
message_id=$(hq ask \
  --details "Option A keeps the old API. Option B removes it." \
  "Should I choose option A or option B?")
```

`hq ask` reads the body from stdin when no body argument is present. Add `--json` for structured output.

## Read the agent mailbox

Use `wait` only when a reply to one message blocks all useful work:

```sh
reply=$(hq wait --timeout 30m "$message_id")
```

Use `poll` when the agent can keep working. `poll` reads replies and unsolicited messages addressed to the current agent session:

```sh
if messages=$(hq poll); then
  printf '%s\n' "$messages"
else
  status=$?
  if [ "$status" -ne 3 ]; then
    exit "$status"
  fi
fi
```

`poll` exits with code 3 and writes nothing when no message is ready. Other errors go to stderr. Plain `wait` output contains only the reply body. Plain `poll` output contains one tab-separated message ID and body per line. Pass `--json` for structured output.

## Delivery rules

`wait` and `poll` lease each message, write the full output once, and then set `completed_at` and `archived_at`. HQ keeps every message. A process crash after stdout but before the database update can cause one later retry, so use the message ID as an idempotency key when a duplicate matters.

Do not use the human `tui`, `list`, `answer`, or `cancel` commands to consume the agent mailbox. Use `wait`, `poll`, or `get`.

Bare `hq` opens the human TUI only when stdin and stdout are terminals. In non-interactive use, bare `hq` lists the open human inbox for the current work directory.
