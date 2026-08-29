# HQ

HQ is a local-first coordination system for durable human, agent, and project work. One Rust
executable provides the CLI, terminal UI, foreground node, SQLite owner, signed causal state,
encrypted Nostr relay synchronization, and managed provider sessions.

## Release status

The first release is `0.1.0`. HQ has not shipped, so this release starts with a
new Rust identity and state directory. It does not migrate, inspect, import, or preserve a Go
database, key, protocol, or command surface. The frozen Go implementation remains design history,
not a supported installation path.

The release supports these native targets:

| Operating system | Architecture | Rust host |
| --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` |
| Linux | ARM64 | `aarch64-unknown-linux-gnu` |
| macOS | Intel | `x86_64-apple-darwin` |
| macOS | Apple Silicon | `aarch64-apple-darwin` |

Windows is not supported in v0.1.0.

## Install a release

Download the archive and checksum for the current machine from the repository's
[GitHub Releases](https://github.com/wbbradley/hq/releases) page.

Verify and install the native archive:

```sh
sha256sum --check hq-v0.1.0-<rust-host>.tar.gz.sha256
tar -xzf hq-v0.1.0-<rust-host>.tar.gz
install -d "$HOME/.local/bin"
install -m 0755 hq "$HOME/.local/bin/hq"
"$HOME/.local/bin/hq" --output json version
```

On macOS, use `shasum -a 256 -c` instead of `sha256sum --check`. The reported commit must equal the
40-character commit targeted by the release tag. The complete artifact and evidence validation
procedure is in
[docs/rust/release-artifacts.md](docs/rust/release-artifacts.md).

## First start

HQ stores state below `$XDG_STATE_HOME/hq`, or `$HOME/.local/state/hq` when `XDG_STATE_HOME` is not
set. An explicit installation can instead use `--state-root ABSOLUTE_PATH` on every invocation.
The directory contains:

- `identity.v1`: the installation's private signing and encryption identity;
- `local-config.v1.json`: unsigned local defaults;
- `hq.sqlite3`: canonical facts and rebuildable projections; and
- `node.lock`: exclusive process ownership.

Create a fresh identity and local human account, then start the TUI:

```sh
hq identity init
hq human create
hq tui
```

Add a retained relay when synchronization is required:

```sh
hq relay add wss://relay.example
hq relay status
```

Run `hq help` for the complete command list and `hq help <command>` for exact syntax. Machine
consumers can put `--output json` before the command to receive `hq-cli-output-v1` records.

## Node and service management

Client commands converge on one state-directory owner and normally auto-start it. These lifecycle
commands are available explicitly:

```sh
hq daemon status
hq daemon readiness
hq daemon stop
hq daemon restart
```

`daemon status` never starts a process. `daemon run` is the foreground role for systemd and
launchd. Multiple HQ processes may be valid when they use different explicit state roots; two
owners can never hold the same `node.lock`. Do not kill processes by name. Inspect their full
arguments and stop only the intended installation with its exact state root.

Supported systemd and launchd installation steps, including absolute executable paths and provider
`PATH` configuration, are in [docs/lan.md](docs/lan.md). The authorization-separated soak and
cutover procedure is in [docs/rust/cutover.md](docs/rust/cutover.md).

## Recovery boundaries

An encrypted `identity export` is the only supported durable backup. The SQLite database is not a
backup artifact: relay-retained signed facts are rebuilt with `relay repair`, while local-only
history is deliberately not promised across node replacement. Never run two nodes with the same
restored identity.

See [docs/rust/recovery.md](docs/rust/recovery.md) for the exact identity backup, database repair,
node replacement, and clean-shutdown contract.

## Provider baseline

The managed Codex adapter is pinned to Codex CLI `0.150.1`. A foreground service must be able to
resolve `codex` from its configured `PATH`; the checked-in service templates include the usual
user-local and system binary directories. The executable protocol contract is documented in
[docs/codex-adapter-v1.md](docs/codex-adapter-v1.md).

## Development

The workspace requires Rust `1.98.0`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features --locked
scripts/verify-rust-qualification.sh --validate-only
```

Build the executable with `cargo build --locked -p hq-node --bin hq`. Release artifacts must be
revision-stamped by setting `HQ_BUILD_COMMIT` to the full Git revision before a locked release
build; the release workflow does this automatically.

The normative rewrite design is [rust-rewrite-design.md](rust-rewrite-design.md). Rust-era
architecture, protocol, behavior, qualification, recovery, and operational documents live under
`docs/rust`, `docs/protocol`, and `docs/adr`.

## License

MIT
