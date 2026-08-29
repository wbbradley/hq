# Rust recovery boundary and rehearsal

The first Rust release supports encrypted installation-identity backup and deterministic repair of
rebuildable SQLite projections. It does not support database-history backup or restoration. These
are deliberately separate boundaries: an identity package preserves authority, while canonical
history must be retained by the node and recovered from authorized peers or relays where available.

## Supported recovery operations

- `hq identity export ABSOLUTE_PATH --password-stdin` creates a new encrypted Rust identity package.
- `hq identity import ABSOLUTE_PATH --password-stdin` imports that authority only into a state root
  with no identity.
- `hq relay repair` reverifies the immutable canonical corpus and atomically rebuilds disposable
  projections. It is repair, not migration, and does not invent missing facts.
- `hq daemon restart` and `hq daemon stop` preserve state and use the ordered lifecycle boundary.

The identity package excludes `hq.sqlite3`, its WAL state, local configuration, relay policy and
delivery state, human accounts, mailboxes, agents, projects, provider sessions, and runtime ledgers.
Copying a live SQLite file or state directory is not a supported backup procedure. The first
release has no database-history restore command. An identity-only replacement therefore starts
with the same installation authority but empty local history; it must catch up through configured,
authorized relay/peer paths before an operator treats it as recovered.

Never run the original and replacement nodes concurrently with one imported identity. Local state
ownership prevents two owners of one directory, but cannot prevent duplicate identity use across
hosts. Keep the original node stopped and archived until the replacement has caught up and a
separate operator decision retires the old installation.

There is no storage-version upgrade or legacy migration path. HQ has never shipped, so the v0.1.0
candidate always initializes new Rust state. A Go key, database, log, or service definition is not
a recovery input and must not be opened by Rust.

## Automated isolated rehearsal

`scripts/rehearse-rust-recovery.sh` accepts an absolute revision-stamped `hq` executable, evidence
output path, complete revision, and native Rust host. It allocates its own temporary rehearsal root
and supplies an explicit state root to every HQ command. It never accepts a state path from the
caller.

The drill:

1. creates a new original Rust identity, local configuration, human account, and named agent;
2. runs explicit database repair and proves the human and agent projections are unchanged;
3. restarts and cleanly stops the original node, then waits boundedly for ownership release;
4. exports the encrypted identity and imports it into a new replacement state root;
5. proves identity equality and proves that configuration and SQLite history were not restored;
6. starts the replacement, observes empty account and agent history, and cleanly stops it; and
7. verifies inaccessible synthetic Go-layout sentinels under a controlled temporary home have
   unchanged identity, modes, sizes, and timestamps.

The synthetic Go sentinels are never passed to HQ, and every product invocation uses an explicit
Rust state root outside that layout. The drill does not open a real Go key/database, inspect a
default user state directory, retain its ephemeral password or identity package, or perform a live
cutover.

Each successful host emits `hq-rust-recovery-rehearsal-v1` JSON. The aggregate validator requires
complete success evidence for all four release hosts and emits `hq-rust-recovery-manifest-v1`.
The separate cutover rehearsal proves an offline selector can return to an untouched synthetic Go
archive only after Rust has stopped, without executing that binary, opening its state, or disturbing
unrelated HQ processes. See [cutover.md](cutover.md).

## Recorded recovery evidence

GitHub Actions run
[33257580370](https://github.com/wbbradley/hq/actions/runs/33257580370) built revision
`f408702866faeeb2530ecedff4a25f9786bea8be` and passed the isolated recovery drill on Linux
x86-64, Linux ARM64, macOS x86-64, and Apple Silicon. Every native record reports exact identity
round-trip, identity-only backup scope, unsupported database-history restoration, successful
projection repair, original restart, replacement startup, clean shutdown, prohibited Go-state
access, and unchanged inaccessible Go sentinels.

The combined artifact is
`rust-release-candidate-f408702866faeeb2530ecedff4a25f9786bea8be`. It was downloaded into a
new temporary directory, where fresh release-artifact and recovery-matrix validation passed. The
regenerated recovery manifest was byte-for-byte equal to the workflow-produced manifest. This is
recovery rehearsal evidence, not authorization to duplicate an identity on live hosts or restore,
replace, or retire an installation.
