# HQ agent command reference

Run `hq agents` first for the normal workflow.

## Ask

```text
hq ask [--session ID] [--dir PATH] [--details TEXT] [--json] [MESSAGE]
```

`ask` reads the message from stdin when no message argument is present. Plain output is the message ID. JSON output is available with `--json`.

## Wait

```text
hq wait [--session ID] [--dir PATH] [--timeout DURATION] [--interval DURATION] [--json] MESSAGE_ID
```

Plain output is the first reply body. `wait` checks that the current mailbox sent the given message.

## Poll

```text
hq poll [--session ID] [--dir PATH] [--json]
```

Plain output contains one tab-separated message ID and body per line. `poll` exits with code 3 and prints nothing when no message is ready. Treat code 3 as an empty mailbox, not an error. Other errors go to stderr.

## Get

```text
hq get MESSAGE_ID
```

`get` writes one known message as JSON without changing it. Use `get` only when a cooperative agent must inspect a message from another mailbox.

HQ detects the current harness session. The `--session` flag and `HQ_SESSION` variable are manual overrides for advanced use. Do not use them in normal work.

Do not use human commands such as `tui`, `list`, `answer`, or `cancel` to read an agent mailbox. Agents also must not run `identity`, `human`, `peer`, `mailbox`, `relay`, `sync`, `daemon`, or `status`; those commands manage the human-owned account, installation, and transport.
