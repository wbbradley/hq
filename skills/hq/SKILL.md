---
name: hq
description: Use HQ for async messages between an agent and a human. Use when work can be queued for the human, a question needs a later answer, the user asks for an HQ message, or the agent needs to check its HQ mailbox.
---

# HQ

Use HQ as the user's preferred async channel. Use the installed `hq` command to send work or questions to the human and to read replies or new messages for the current agent session. Act on requests to use HQ instead of only showing commands.

Before the first HQ action in a task, run:

```sh
hq agents
```

Treat that output as the current guide. It lists focused help topics for details such as command output, sync, and delivery behavior. If `hq` is missing or reports a setup error, report the error. Do not install or configure HQ unless the user asks.
