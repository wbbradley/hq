# Rust node lifecycle and ownership foundation

Status: Active foundation contract

The Rust node is the only composition root. Before any listener, relay, managed runtime, or project
worker starts, one `NodeFoundation` acquires these owners in order:

1. the process-lifetime `node.lock` for the selected state root;
2. the validated private installation identity;
3. the validated unsigned local configuration;
4. the private runtime directory; and
5. the bounded synchronous store actor.

Each step returns only typed redacted failures. An early return drops every earlier owner, so a
corrected startup can immediately reacquire the lock. Normal checked shutdown closes and joins the
store before the runtime, identity, and state-lock owners are dropped. `Drop` repeats the same
idempotent best-effort containment for panic and early-return paths; callers use checked shutdown
when failure reporting matters.

## Runtime namespace

The runtime namespace is distinct from durable identity and storage. `RuntimePaths::new` accepts an
explicit nonempty absolute root. `RuntimePaths::derive` otherwise selects:

- `$XDG_RUNTIME_DIR/hq/<installation qualifier>` when an explicit XDG runtime input is available;
  or
- `<state root>/runtime` as the macOS/service-manager fallback.

The installation qualifier is the lowercase hexadecimal first 96 bits of the installation ID. It
is collision resistance for a non-authoritative local pathname, not an identity or authorization
input; the state lock and later peer/session checks remain authoritative for ownership. A qualifier
collision fails safely at listener binding.

The root is exactly `0700`, may not be a symbolic link, and owns reserved `node.sock` and
`node-ready.v1.json` paths. Reserved artifacts may not be symbolic links. A portable first-release
limit of 103 pathname bytes reserves the terminating NUL in macOS's 104-byte `sockaddr_un.sun_path`
while also fitting Linux. Construction rejects a longer socket path before filesystem or bind work.

Runtime-directory preparation creates or validates the root but never removes a socket, readiness
file, or other stale artifact. Only the later listener package may replace stale runtime artifacts,
and only after it holds the installation state lock and proves that no live listener owns them.

## Lifecycle and admission

The pure lifecycle has five closed phases:

| Phase | Meaning | Status | Queries | Mutations | Launches |
| --- | --- | ---: | ---: | ---: | ---: |
| `Starting` | required owners/components are not yet acknowledged | yes | no | no | no |
| `Ready` | required owners/components are available at a serialized store revision | yes | yes | yes | yes |
| `Draining` | new side effects are closed while accepted work drains | yes | yes | no | no |
| `Failed` | a stable startup/runtime failure is retained | yes | no | no | no |
| `Stopped` | all ownership has been acknowledged closed | no | no | no | no |

Readiness is legal only from `Starting` and records the store's authoritative current revision. A
stop request enters `Draining` with `Stop` intent. A restart request is legal only from `Ready`,
enters the same drain phase with `Restart` intent, and does not itself start a replacement process.
Repeated identical drain, restart, and stopped acknowledgements are idempotent. Out-of-order
readiness/restart/stopped events return a typed lifecycle error.

Startup diagnostics contain a closed component, cause, and suggested operator action plus only the
state/runtime roots explicitly selected by the caller. They do not retain operating-system,
SQLite, key, configuration, or adapter prose. Safe public installation metadata is available
separately; signer secret bytes have no diagnostic path.

## Executable contracts

`crates/hq-node/tests/node_foundation.rs` covers absolute and installation-qualified derivation,
portable length rejection, private modes, symbolic links, non-destructive stale artifacts, every
lifecycle admission transition, out-of-order restart/readiness, concurrent state ownership,
missing identity, unsafe runtime, store-open rollback, mutation rejection during drain, checked
store close, immediate lock reacquisition, and redacted debug surfaces.
