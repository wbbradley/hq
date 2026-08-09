# HQ instructions for agents

HQ lets an agent send a message to a human and read messages from its agent mailbox later.

## Mailbox scope

HQ finds the current Codex, Claude Code, or Pi session and binds one private mailbox to that harness session. The mailbox stays the same after a process restart or directory change when the harness resumes the same session. Do not create, print, save, or pass a session ID in normal use.

If HQ reports an ambiguous or missing harness session, stop and report the error. The `--session` flag and `HQ_SESSION` variable are manual overrides for advanced use only.

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

Use `poll` when the agent can keep working. `poll` reads replies and unsolicited messages addressed to the current harness mailbox, even when the work directory has changed:

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

`wait` checks that the current mailbox sent the given message. `get MESSAGE_ID` is the direct-ID path for a cooperative agent that must inspect a message from another mailbox.

HQ threads may contain more than one answer. `wait` returns the first answer available to the current mailbox, not a globally selected answer. Use `poll` to read later answers and async messages.

Network events may arrive before their causal parents. `poll` and `get` can show an addressed message with incomplete causal history, but `wait` requires the original local question so HQ can prove mailbox ownership. Keep the event ID and treat a later copy as the same event.

Cancellation does not erase an answer. A thread may show both facts when an answer arrived after the sender cancelled or when answer and cancellation were concurrent. Read the causal status and handle the answer if it still helps; do not assume that the answerer saw the cancellation.

Bare `hq` opens the human TUI only when stdin and stdout are terminals. In non-interactive use, bare `hq` lists the open human inbox for the current work directory.
