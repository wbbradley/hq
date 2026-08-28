# Rust workspace and dependency contract

Status: Active foundation contract

The Rust rewrite lives beside the frozen Go implementation until an authorized cutover. The root
Cargo workspace uses Rust 1.98.0, edition 2024, resolver 3, shared formatting, and deny-level Rust
and Clippy policy. Third-party runtime dependencies remain confined to the adapters that own them;
in particular, only `hq-relay` imports bounded blocking Tungstenite/Rustls WebSocket transport.
`crates/hq-node` owns the
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
| `hq-resources` | Path identity plus bounded filesystem and Git observation | Domain |
| `hq-harness` | Provider-neutral runtime contract, registry, buffer, and supervisor | Domain |
| `hq-projects` | Explicit durable project command workflows and strict remote command codec | Application, domain, reducer, harness, resources |
| `hq-codex` | Private Codex adapter | Domain, `hq-harness` |
| `hq-tui` | Pure UI state plus terminal adapter | Domain, application |
| `hq-node` | Composition, runtime ownership, single binary | Any inward crate |
| `hq-testkit` | Deterministic builders, reusable conformance, and scripted adapters | Domain, harness; reducer in tests |

An allowlist in `scripts/verify-rust-architecture.sh` enforces direct internal dependencies. The
same verifier rejects Tokio, SQLite, Nostr, Ratatui, filesystem, process, and provider-specific
concerns in `hq-domain`, `hq-reducer`, and `hq-application`; rejects provider-specific vocabulary and
serialization/runtime/process/filesystem dependencies in `hq-harness`; and requires the
provider dependency to point from `hq-codex` to the neutral harness contract. Provider-neutral
identities remain valid domain vocabulary. This source scan complements
Cargo's cycle checks: dependency acyclicity alone does not prove that a core crate is pure.

`hq-harness` implements the synchronous object-safe boundary in
`docs/harness-contract-v1.md`: passive capability and event records have public fields, while the
registry owns namespace and safe-recovery invariants and mutable traits represent actual runtime
capabilities. Its operational ownership and recovery contract is
`docs/harness-supervisor-v1.md`; passive supervisor records also expose fields directly, while exact
owner tokens and copied secret environments remain opaque. `hq-testkit` owns the reusable scenario driver and deterministic scripted adapter in
`docs/testing/conformance-v1.md`. Production adapters depend inward on the neutral contract; the
neutral contract never imports a provider adapter or its wire vocabulary.

`hq-codex` implements the pinned provider boundary in `docs/codex-adapter-v1.md`. Passive launch and
factory configuration use public fields; child ownership, RPC identities, recovery maps, pending
interactive requests, and mutable session state remain opaque capabilities. Its private synchronous
JSONL/process implementation adds no async runtime and no Codex DTO or method name crosses into a
neutral crate.

`hq-resources` implements `docs/path-resources-v1.md`. Passive requests and reports expose public
fields. The adapter itself remains an opaque capability because it owns injected filesystem and Git
effects. It preserves normalized human spelling separately from immutable canonical identity,
keeps home qualification explicit, and returns closed health/release evidence without retaining
file names, contents, stderr, environment, or operating-system diagnostics.

`hq-projects` owns passive public command/checkpoint records, a strict canonical versioned command
body codec, exact-replay intake, transaction-consistent canonical project mutation decisions,
session-free configuring intent, activation/compensation, and at-most-once pending-input dispatch.
It does not own SQLite or provider processes. `hq-node` maps its checkpoint capability to
store-owned v13 records and maps project runtime operations to the neutral harness supervisor's
sole durable delivery ledger; explicit workflow handlers inject canonical, runtime, resource, and
Git capabilities without reversing dependencies.

The in-memory composition path remains intentionally non-normative at the protocol boundary: it
validates a small frame in `hq-protocol`, constructs an `hq-domain` fact, and submits it through
`hq-application`. Reduction now uses the normative pure complete-batch causal kernel in
`hq-reducer`; later domain packages plug authorization, aggregate, and projection policy into that
kernel without moving graph logic into application or adapter crates.

`hq-application` owns normalized projection snapshots, strict retry outcomes, effect requests, and
the consumer-side capability traits documented in `docs/rust/application-services.md`. Storage,
local sessions, relays, managed runtimes, resource observers, and the node implement or compose
those ports; application services never import their concrete types.

`hq-local-api` owns the independently versioned local API v1 DTOs, canonical JSON codec, bounded
length framing, negotiation values, exact mutation-plan replay representation, lifecycle/domain/
effect request families, client snapshot/page values, typed errors, and revision-only invalidations
specified in `docs/protocol/local-api-v1.md`. Its unsigned semantic-plan bridge reuses the canonical
protocol owner's exhaustive semantic spelling, but the bytes carry no authority and must still pass
ordinary node signing and canonical verification. No local API production source imports storage.
The transport-independent server session retains only connection protocol state and its shared
revision registrations. It borrows application and lifecycle capabilities for one decoded request
dispatch and never lifts concrete node owners into reference-counted task state. The session accepts
only one unconfirmed response write, activates a pending subscription from that response's
session-owned confirmation ticket, and cancels every owned registration on disconnect. Its shared
revision hub has a fixed registration capacity and one in-place coalesced invalidation per slow
subscriber. Application owns the closed normalized client projection catalog; the local API
performs the only conversion from that catalog to wire DTOs and does not import reducer or storage
crates.

The same crate owns one transport-independent reconnecting client state machine for the CLI, TUI,
and harness launchers. A narrow adapter performs only connect, complete-frame write, and idempotent
close operations; the pure machine emits generation-scoped actions and deterministic capped
backoff delays. It renegotiates every connection, replays retained mutation frames byte-for-byte,
reports lost ordinary requests without replaying them, derives a fresh subscription registration
per server session, and treats revision notices only as wakes for complete authoritative refreshes.
All retained mutation and completed-identity state is explicitly bounded.

`hq-node` owns the secure lifecycle foundation specified in `docs/rust/node-lifecycle.md`. It
derives or accepts a private runtime namespace, enforces the portable Unix socket pathname ceiling,
and composes the state lock, identity/configuration, runtime directory, and bounded store actor in
one RAII owner. Its pure lifecycle closes mutation and launch admission at drain entry, retains
explicit stop/restart intent, and publishes readiness only with a serialized store revision.
Startup failures carry closed component/cause/action values and selected paths without retaining
secret, SQLite, or operating-system diagnostics. The node now also owns the four-slot component
lifecycle catalog, hierarchical cancellation, fixed-capacity tracked threads and nonblocking
mailboxes, ordered rollback/drain/escalation, shared revision hub, and the transient delegated
application capability bundle. The foundation now exclusively binds and owns the private `0600`
Unix socket, probes identity-stable stale sockets without blocking, validates Linux/macOS same-user
peer credentials, atomically publishes strict bounded readiness metadata, and removes only exact
owned runtime identities. An authenticated accepted stream is an opaque capability consumed by one
Tokio-owned bounded session I/O future. The future incrementally decodes into a caller-bounded event
channel, writes from a fixed encoded-frame queue, emits response tickets only after complete frame
writes, and joins its read/write halves into one exact terminal event. Listener multiplexing and
lifecycle coordination remain in the immediately following node package.

The node also owns `HarnessStoreAdapter`, the only mapping between neutral supervisor records and
storage-owned records, plus `HarnessNodeComponent`, which composes the registry, restricted store
handle, canonical persistence capability, injected clock/token sources, and ordered lifecycle.
Application harness control cannot obtain a provider session or SQLite handle directly.

## Supported target matrix

ADR 0001 defines four first-release targets:

| Operating system | Architecture | Rust target |
| --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` |
| Linux | ARM64 | `aarch64-unknown-linux-gnu` |
| macOS | x86-64 | `x86_64-apple-darwin` |
| macOS | Apple Silicon | `aarch64-apple-darwin` |

CI runs the complete Rust workspace natively on Linux and macOS, cross-checks the pure core,
application, protocol, neutral harness, reusable conformance boundary, and standard-library Codex
and path-resource adapters for all four triples, and runs pinned protocol fuzz smoke gates on Linux. A cross-target
check is compilation evidence, not an installed-provider or lifecycle test.
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
