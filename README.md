# HQ

HQ lets an agent ask a person a question, keep working, and later poll or wait for the answer. A terminal UI gives the person one queue for all local agents.

HQ stores each question in a local SQLite database. The default path is `~/.local/state/hq/hq.db`, or `$XDG_STATE_HOME/hq/hq.db` when `XDG_STATE_HOME` is set.

## Install

HQ needs Go 1.25 or later.

```sh
go install github.com/wbbradley/hq/cmd/hq@latest
```

## Agent use

Run `hq agents` to print agent-specific setup, command, output, and delivery rules:

```sh
hq agents
```

HQ embeds [the agent instruction source](internal/agenthelp/instructions.md) in the binary. Edit that file to update both the checked-in source and `hq agents` output.

HQ resolves an agent session from `HQ_SESSION`, then `CODEX_THREAD_ID`. An explicit `--session` flag takes priority.

## Human use

Run `hq` with no command in a terminal to open the terminal UI:

```sh
hq
```

When stdin or stdout is not a terminal, bare `hq` lists only pending questions in the current work dir and inferred session. If HQ cannot infer a session, the list includes all sessions in the current work dir. Use explicit `hq list` flags for another scope or status.

Use `j` and `k` to move, Enter to answer, Ctrl+S to submit, and `q` to quit. The human commands also work without a TTY:

```sh
hq list --status pending
hq answer 0198c7ec-73b0-7cc3-a5f7-e31c77140d65 "Use ListWidgets"
hq cancel 0198c7ec-73b0-7cc3-a5f7-e31c77140d65
```

## Command summary

```text
hq ask [--session ID] [--dir PATH] [--details TEXT] [--json] [PROMPT]
hq wait [--timeout DURATION] [--interval DURATION] [--json] QUESTION_ID
hq poll [--session ID] [--dir PATH] [--json]
hq get QUESTION_ID
hq list [--session ID] [--dir PATH] [--status STATUS] [--limit N] [--json]
hq answer QUESTION_ID [RESPONSE]
hq cancel QUESTION_ID
hq tui
hq agents
```

Set `HQ_DB` or pass the global `--db PATH` flag before the command to use another database.

## Data and delivery rules

HQ keeps all question rows. A row moves from `pending` to `answered` or `cancelled`. A successful `wait` or `poll` sets `completed_at`; HQ does not delete the row.

HQ leases an answered row before delivery, so two consumers cannot print the same answer at the same time. A failed write releases the lease. A process crash between stdout and the final database update can cause a later retry to print the answer again. Consumers should use the question ID as an idempotency key when that rare duplicate matters.

See [docs/design.md](docs/design.md) for the schema and storage contract.

## Development

```sh
go test ./...
go vet ./...
go build ./cmd/hq
```

Releases use GoReleaser when a `v*` tag reaches GitHub. CI tests Linux, macOS, and Windows.

## License

MIT
