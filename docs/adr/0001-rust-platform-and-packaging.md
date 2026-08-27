# ADR 0001: Rust platform and packaging boundary

Status: Accepted

Date: 2026-08-26

## Context

The Rust rewrite needs a supported local-IPC and process-lifecycle surface before workspace,
local-API, node, packaging, and release work begin. The Go tree contains Unix socket behavior,
systemd and launchd examples, Windows-specific source files, and Windows CI, but explicitly lacks
a Windows local client transport. Preserving build targets without a usable node/client path would
create a misleading support claim.

The node and CLI are architectural boundaries, but they do not have to be separate installed
executables. Autostart, version matching, diagnostics, and operator installation are simpler when
one installed artifact contains both roles.

## Decision

The first complete Rust release supports Linux and macOS as first-class operating systems:

- Linux on x86-64 and ARM64, using Unix domain sockets, Unix ownership and permission checks,
  signals, and systemd user-service guidance.
- macOS on Apple Silicon and x86-64, using the same Unix contract and launchd guidance.
- Filesystem durability and permission tests must run on both operating systems. Platform-specific
  behavior must sit behind adapters rather than enter the domain or reducer.

Windows support is deferred. A Windows release cannot be described as supported until it has a
same-user named-pipe local transport, ownership locking, secure state/runtime path policy, process
shutdown semantics, CI, and lifecycle acceptance evidence equivalent to the Unix path. Core crates
should remain portable where that costs little, but Windows compilation is not a first-release
gate.

HQ ships one user-facing executable named `hq`. It contains thin client commands, the Ratatui
client, lifecycle control, and an explicit foreground node entry point used by autostart and service
managers. Library/crate boundaries remain separate; this is only a distribution choice. The node
is still the sole composition root and normal client operations never open the store, sign facts,
own relay sessions, or supervise providers.

Rust uses a separately derived state, configuration, and runtime namespace and refuses Go files.
The exact path spelling is owned by the installation-identity specification, but it must permit
controlled Go and Rust dogfood identities to coexist without opening the same key, database,
socket, lock, or metadata file.

## Consequences

- Local API and node work may standardize on a Unix-domain-socket implementation first.
- Release automation has four first-class OS/architecture targets and does not advertise Windows.
- One installed artifact simplifies autostart and build-version diagnostics while crate boundaries
  preserve decoupling.
- Adding Windows later is a feature package with its own lifecycle and security evidence, not a
  compile-only checkbox.
- No decision here preserves Go command spelling, daemon modes, socket names, deployment file
  contents, or path layout.
