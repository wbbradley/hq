# HQ

HQ is a local message system for agents and a human. Agents send messages to the human inbox, the human replies from a terminal UI, and agents read replies or other mailbox messages later.

HQ stores messages in SQLite at `~/.local/state/hq/hq.db`, or `$XDG_STATE_HOME/hq/hq.db` when `XDG_STATE_HOME` is set.

## Install

HQ needs Go 1.25 or later.

```sh
go install github.com/wbbradley/hq/cmd/hq@latest
```

From a local checkout:

```sh
go install ./cmd/hq
```

Create one installation identity before first use:

```sh
hq identity init
```

HQ stores the root key in `~/.local/state/hq/hq.key` with mode `0600` and keeps the key out of SQLite. Same-user processes can still read the key, so this release assumes cooperative local actors.

## Agent use

Install the HQ agent skill with GitHub CLI:

```sh
gh skill install wbbradley/hq hq@main --agent codex --scope user
```

The `@main` pin installs the current skill before the next tagged HQ release. After a release includes the skill, `hq@main` can be shortened to `hq`.

Run `hq agents` to print agent-specific setup, output, and delivery rules:

```sh
hq agents
```

HQ embeds [the agent instruction source](internal/agenthelp/instructions.md) in the binary. Edit that file to update both the source and `hq agents` output.

HQ detects `CODEX_THREAD_ID`, `CLAUDE_CODE_SESSION_ID`, or `PI_SESSION_ID` and binds one private mailbox to that namespaced harness session. Agents do not need to manage a session ID. HQ stops with an error if more than one built-in harness ID is present because silent routing could select the wrong mailbox.

```sh
message_id=$(hq ask "Which API name should I use?")
reply=$(hq wait --timeout 30m "$message_id")

# Read replies and unsolicited messages in this agent mailbox.
hq poll
```

`poll` exits with code 3 and writes nothing when the mailbox has no ready messages.

The mailbox follows a resumed harness session across process restarts and directory changes. `--session ID` and `HQ_SESSION` select a `custom` mailbox for advanced use; an explicit flag wins. `hq mailboxes` lists mailbox candidates seen in the current directory, Git common directory, worktree, branch, or compact remote identity. The command does not claim or merge a mailbox.

## Human use

Run `hq` in a terminal to open the mailbox UI:

```sh
hq
```

The default view shows open messages in the reserved `human` mailbox. Use these keys:

- `j` / `k`: move
- Enter: reply to an open inbox message; Enter again submits
- `n`: write a new message to the agent session tied to the selected row
- Shift+Enter / Ctrl+J: add a line break while editing
- `s`: toggle sent messages
- `x`: toggle archived inbox messages
- `v`: toggle relay and event status
- `r`: refresh
- `q`: quit

Sent and Archived are independent filters. This lets the human view the open inbox, add sent messages, add archived messages, or show all three sets together. Each detail panel also shows the local git branch, compact remotes, and an asynchronously loaded open pull request when `gh` credentials are available.

When stdin or stdout is not a terminal, bare `hq` lists open messages in the human mailbox for the current work directory.

Human commands also work without the TUI:

```sh
hq list --recipient human
hq answer MESSAGE_ID "Use ListWidgets"
hq cancel MESSAGE_ID
```

## Command summary

```text
hq ask [--session ID] [--dir PATH] [--details TEXT] [--json] [MESSAGE]
hq wait [--session ID] [--dir PATH] [--timeout DURATION] [--interval DURATION] [--json] MESSAGE_ID
hq poll [--session ID] [--dir PATH] [--json]
hq get MESSAGE_ID
hq list [--sender MAILBOX] [--recipient MAILBOX] [--dir PATH] [--archived|--all] [--limit N] [--json]
hq mailboxes [--dir PATH] [--json]
hq identity init
hq identity show [--json]
hq identity export BACKUP_PATH
hq identity import BACKUP_PATH
hq identity reset --yes
hq peer add [--name NAME] [--relay URL] INSTALLATION_ID NPUB
hq peer list [--json]
hq peer distrust INSTALLATION_ID
hq mailbox share MAILBOX_ID PEER_INSTALLATION_ID
hq mailbox revoke MAILBOX_ID PEER_INSTALLATION_ID
hq relay add [--read=BOOL] [--write=BOOL] [--unsafe-no-auth] WSS_URL
hq relay list [--json]
hq relay remove WSS_URL
hq status [--json]
hq sync
hq daemon run|status|stop
hq answer MESSAGE_ID [RESPONSE]
hq cancel MESSAGE_ID
hq tui
hq agents
```

Set `HQ_DB` or pass global `--db PATH` before the command to use another database. Mutating commands commit their signed local event and then run a three-second foreground sync pass. `--no-sync` skips that pass for explicit offline work. Relay errors go to stderr and never undo the local event.

No daemon is required. `hq daemon run` is an optional foreground service for continuous polling; a service manager may keep it alive. `hq daemon status` reads its protected local socket, and `hq daemon stop` requests a clean stop. A CLI send wakes the daemon when the daemon owns the sync lock. `hq sync` runs one full pass when no daemon owns the lock.

## Message and delivery rules

Each mailbox has one opaque ID. An agent mailbox has a unique `(harness, external session ID)` binding. The reserved human mailbox is installation-wide. Signed message and mailbox-context events carry directory and Git data; those fields aid display and abandoned-mailbox search but do not grant mailbox access. Replying adds signed answer and archive events in one SQLite transaction.

`wait` reads a reply only when the current mailbox sent the first message. `poll` reads every ready message addressed to the current harness mailbox, including unsolicited human messages, without a directory filter. `get` keeps direct-ID access as an explicit path for cooperative cross-mailbox inspection. Delivery leases each row, writes stdout once, and then sets `completed_at` and `archived_at`. A crash after stdout but before the database update can cause one later retry, so consumers can use the message ID as an idempotency key.

`hq list` shows only open messages by default. `--archived` shows archived messages, and `--all` shows both.

The TUI syncs in the background on start, after a reply, and during its one-minute refresh. Active text, focus, and selection survive the reload. Sent rows show `sending`, `sent`, `peer received`, or `rejected`. Press `v` for relay health and queue, unresolved, unsupported, staging, and quarantine counts.

Schema version 5 resets every older HQ table when HQ first opens an old database. HQ is still in green-field development and does not migrate old rows.

See [docs/design.md](docs/design.md) for the storage contract, [docs/events.md](docs/events.md) for signed causal state, and [docs/nostr.md](docs/nostr.md) for encrypted relay transport.

## Development

```sh
go test ./...
go vet ./...
go build ./cmd/hq
```

Releases use GoReleaser when a `v*` tag reaches GitHub. CI tests Linux, macOS, and Windows.

## License

MIT
