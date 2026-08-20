---
name: hq
description: Use HQ for request-response questions and asynchronous messages between an agent and a human. Use when an agent needs a human answer, work can be queued for the human, the user asks for an HQ message, or the agent needs to check its HQ mailbox.
---

# HQ

Use HQ as the user's preferred human contact channel. Use `hq ask` for questions: it waits indefinitely until the human replies. Do not add a timeout unless the human supplied a real deadline. Use `hq send` only for fire-and-forget messages or when useful work can continue asynchronously. Act on requests to use HQ instead of only showing commands.

Before the first HQ action in a task, run:

```sh
hq agents
```

Treat that output as the current guide. It lists focused help topics for details such as command output, sync, and delivery behavior. If `hq` is missing or reports a setup error, report the error. Do not install or configure HQ unless the user asks.

HQ may route a human reply from any paired account device back to the current harness mailbox. `hq ask` handles the reply directly; `hq send` prints a message ID for later `hq wait`. Do not infer or manage the human device route.
