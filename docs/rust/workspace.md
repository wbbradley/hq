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

The current in-memory skeleton is intentionally non-normative. It validates a small frame in
`hq-protocol`, constructs an `hq-domain` fact, submits it through `hq-application`, derives a stable
summary in `hq-reducer`, and composes the path in `hq-node`. It proves dependency direction and
test placement only. The domain and causal-kernel packages replace these small shapes with the
cataloged semantics.

## Supported target matrix

ADR 0001 defines four first-release targets:

| Operating system | Architecture | Rust target |
| --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` |
| Linux | ARM64 | `aarch64-unknown-linux-gnu` |
| macOS | x86-64 | `x86_64-apple-darwin` |
| macOS | Apple Silicon | `aarch64-apple-darwin` |

CI runs the complete Rust workspace natively on Linux and macOS and cross-checks the pure core for
all four triples. A cross-target check is compilation evidence, not an adapter or lifecycle test.
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
cargo deny check
```

`deny.toml` rejects advisories, wildcard dependency versions, unknown registries, unknown Git
sources, and licenses outside the recorded allowlist; duplicate versions are visible warnings.
CI pins cargo-deny 0.20.2. When adding a dependency, put it in the narrowest adapter that owns the
capability, justify every new license or source, and update the architecture allowlist only when
the intended direction in this document changes.
