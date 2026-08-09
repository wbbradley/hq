---
name: hq
description: Send messages from an agent session to a human with the HQ CLI, wait for replies, and read unsolicited mailbox messages. Use when the user asks the agent to contact or consult the human through HQ, send an HQ message, check the agent's HQ inbox, wait for an HQ reply, or use HQ during a long-running task.
---

# HQ

Use the installed `hq` command to exchange messages with the human mailbox. Act on the request instead of only showing commands when the user asks to send, wait, or check the inbox.

## Load the current command guide

Before the first HQ action in a task, check that `hq` is on `PATH`, then run:

```sh
hq agents
```

Treat that output as the source of truth for session scope, commands, output, exit codes, and delivery rules. Do not rely on command details copied from an older conversation.

If `hq` is missing, report that the HQ binary is required. Do not install software unless the user asks.

## Choose the action

- Send a message with `hq ask`. Write a clear body, save the message ID, and report the ID to the user.
- Check the current agent mailbox with `hq poll`. Treat exit code 3 as an empty mailbox, not as a failure.
- Use `hq wait` only when a reply blocks all useful work. Prefer continued work plus later polling when other work remains.
- Let HQ detect the harness session. Do not create, print, save, or pass a session ID in normal use.
- Follow the duplicate-delivery guidance printed by `hq agents` when message IDs control side effects.

Do not open the human TUI or use human mailbox commands to consume an agent mailbox.
