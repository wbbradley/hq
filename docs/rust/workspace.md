# Rust workspace and dependency contract

Status: Active foundation contract

The Rust rewrite lives beside the frozen Go implementation until an authorized cutover. The root
Cargo workspace uses Rust 1.98.0, edition 2024, resolver 3, shared formatting, and deny-level Rust
and Clippy policy. It currently has no third-party runtime dependency. `crates/hq-node` owns the
only binary target, named `hq`; this records the single-executable packaging decision without
making the skeleton a supported replacement for the Go executable.

## Boundaries

| Crate | Owns | May depend inward on |
| --- | --- | --- |
| `hq-domain` | Pure values and policies | Standard library only |
| `hq-reducer` | Pure causal graph and projection logic | `hq-domain` |
| `hq-protocol` | Canonical and remote-control byte transitions | `hq-domain` |
| `hq-application` | Use cases and ports | `hq-domain`, `hq-reducer` |
| `hq-store` | Durable state and commit adapter | Domain, reducer, protocol, application |
| `hq-local-api` | Local client protocol and sessions | Domain, protocol, application |
| `hq-relay` | Encrypted relay transport | Domain, protocol, application |
| `hq-harness` | Provider-neutral runtime contract | Domain, application |
| `hq-codex` | Private Codex adapter | Domain, application, `hq-harness` |
| `hq-tui` | Pure UI state plus terminal adapter | Domain, application |
| `hq-node` | Composition, runtime ownership, single binary | Any inward crate |
| `hq-testkit` | Deterministic builders and scripted adapters | Pure and application crates |

An allowlist in `scripts/verify-rust-architecture.sh` enforces direct internal dependencies. The
same verifier rejects Tokio, SQLite, Nostr, Ratatui, filesystem, process, and provider-specific
concerns in `hq-domain` and `hq-reducer`; rejects Codex vocabulary in `hq-harness`; and requires the
provider dependency to point from `hq-codex` to the neutral harness contract. Provider-neutral
identities remain valid domain vocabulary. This source scan complements
Cargo's cycle checks: dependency acyclicity alone does not prove that a core crate is pure.

The in-memory composition path remains intentionally non-normative at the protocol boundary: it
validates a small frame in `hq-protocol`, constructs an `hq-domain` fact, and submits it through
`hq-application`. Reduction now uses the normative pure complete-batch causal kernel in
`hq-reducer`; later domain packages plug authorization, aggregate, and projection policy into that
kernel without moving graph logic into application or adapter crates.

## Supported target matrix

ADR 0001 defines four first-release targets:

| Operating system | Architecture | Rust target |
| --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` |
| Linux | ARM64 | `aarch64-unknown-linux-gnu` |
| macOS | x86-64 | `x86_64-apple-darwin` |
| macOS | Apple Silicon | `aarch64-apple-darwin` |

CI runs the complete Rust workspace natively on Linux and macOS, cross-checks the pure core and
pure-Rust protocol boundary for all four triples, and runs a pinned signed-event fuzz smoke gate on
Linux. A cross-target check is compilation evidence, not an adapter or lifecycle test.
Windows is deliberately absent: inexpensive core portability is welcome, but product support
requires the separate local-transport, ownership, lifecycle, path-policy, and acceptance work in
ADR 0001.

## Contributor gates

Run these commands from the repository root:

```sh
cargo fmt --all -- --check
scripts/verify-rust-architecture.sh
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets --all-features
cargo build --locked --workspace --all-targets --all-features
scripts/verify-rust-dependencies.sh
scripts/verify-rust-protocol-fuzz.sh
```

The root and isolated fuzz policies reject advisories, wildcard dependency versions, unknown
registries, unknown Git sources, and licenses outside their recorded allowlists; duplicate versions
are visible warnings. CI pins cargo-deny 0.20.2. When adding a dependency, put it in the narrowest
adapter that owns the capability, justify every new license or source, and update the architecture
allowlist only when the intended direction in this document changes.
