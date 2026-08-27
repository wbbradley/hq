# Reimplementing HQ in Rust

Status: historical feasibility, risk, and verification analysis written against the pre-release
Go implementation on 2026-08-26. `rust-rewrite-design.md` and `PLAN.md` now govern the clean-sheet
rewrite. Where this document suggests Go-oracle or differential compatibility work, their source
precedence and explicit non-goals control instead.

## Executive conclusion

HQ is a good candidate for Rust, but not because Rust would make every part of it shorter. The
project's essential complexity is real: it is a signed causal event system, a transactional
projection engine, a relay client, a process supervisor, a local RPC server, and a TUI. Rust cannot
remove those responsibilities.

Rust would, however, make the most correctness-sensitive parts substantially easier to represent
and refactor. Algebraic data types, exhaustive matching, newtypes, `Result`/`Option`, ownership, and
traits would let the compiler enforce distinctions that the Go implementation currently enforces
through conventions, validation functions, string constants, and tests. That is especially
valuable in the canonical wire model, reducer decisions, capabilities, project transitions,
conversation entries, mutation outcomes, and supervisor lifecycle.

The likely long-term result is:

- The protocol and pure reducer become less verbose, more explicit, and safer to extend.
- Storage code becomes somewhat safer but not dramatically smaller; SQL and transaction ordering
  remain intrinsically verbose.
- Async lifecycle code becomes more strongly checked but not automatically simpler.
- The TUI may initially be *more* code in Ratatui than in Bubble Tea because Ratatui provides
  rendering primitives, not an application architecture or input loop.
- Build times, compiler friction, and generic/async error messages become a new iteration cost.

Because conversion time is not the deciding concern, backwards compatibility is explicitly not a
goal, and HQ is still pre-release, I would choose Rust for the long-lived implementation. I would
do so only as a verification-led replacement: first extract an executable behavioral specification
from Go and the design documents, then build the Rust implementation against that specification.
A line-by-line translation followed by informal dogfooding would be too risky.

The most important distinction is that **clean-break compatibility does not mean unverified
behavior**. The Rust version may use a new database, RPC protocol, canonical schema, and state
directory while still proving that intended domain behavior and safety properties survived.

## What is being replaced

The present codebase contains approximately 37,000 lines of non-test Go and 24,000 lines of Go
tests across 194 files. The largest production areas are approximately:

| Area | Non-test Go lines | Why it matters to the port |
| --- | ---: | --- |
| SQLite store and projections | 8,800 | Transactionality, rebuilds, outbox, receipts, projects |
| TUI | 4,800 | Input, rendering, drafts, selection, reconnection |
| Codex bridge | 4,600 | JSON-RPC, process I/O, correlation, backpressure |
| Harness bridge/supervisor | 3,700 | Ownership, cancellation, lifecycle, persistence |
| CLI | 2,000 | Command surface and error presentation |
| Relay synchronization | 1,600 | Retry, subscription, delivery, shutdown |
| Canonical reducer and wire model | 3,000 | Signature, validation, causal semantics |

These counts are not estimates of Rust size. They show where behavioral risk is concentrated. In
particular, “porting the TUI” accounts for only a fraction of the work.

The current architectural contracts in [docs/design.md](docs/design.md),
[docs/events.md](docs/events.md), [docs/nostr.md](docs/nostr.md), and
[docs/projects.md](docs/projects.md) include:

- One local node owns the installation key, SQLite, reducer, projections, relay connections,
  subscriptions, and Codex children.
- Exact signed canonical event bytes are the source of truth; projections are rebuildable.
- Event reduction is deterministic, idempotent, causally ordered, and independent of receipt order.
- Incremental projection must agree with a whole-log rebuild.
- Durable mutations have request identity, receipts, retry safety, outbox derivation, revision
  publication, and one transaction boundary.
- Relay wrappers, retries, staging, quarantine, and delivery records have different trust and
  durability roles from canonical events.
- The CLI and TUI are reconnecting RPC clients and do not directly own the database or workers.
- The supervisor must coordinate child processes, bounded output, durable submissions, cancellation,
  and graceful drains without leaking caller environments.

Those contracts, not the current package or file layout, are the real porting target.

## Assumptions and non-goals

This proposal assumes:

- The TUI will use Ratatui, normally with Crossterm for terminal events.
- The Rust version gets a new state directory, database schema, local RPC protocol, and canonical
  schema version.
- The Rust version need not open a Go database or communicate with a Go node.
- Existing users may start with a new identity and empty state. A one-shot data export/import tool
  would be a separate product decision, not a compatibility requirement.
- The Go implementation can be retained temporarily as a test oracle and read-only fallback.
- No live Go and Rust nodes will concurrently use the same installation key or relay identity.

Non-goals are source-level similarity, preservation of Go package boundaries, byte-identical local
RPC, and pixel-identical TUI rendering.

## Where the port is difficult

### 1. Canonical bytes, signatures, and encrypted transport

HQ's wire layer has exact-byte behavior: strict JSON, NIP-01 event IDs, BIP-340 signatures,
bounded payloads, causal references, NIP-44 encryption, and NIP-59 gift wrapping. A seemingly
harmless serializer change can alter an event ID. A permissive decoder can turn rejected input into
accepted state. A retry that regenerates an ephemeral wrapper can violate the exact-wrap reuse
contract.

Serde can express tagged enums and reject unknown fields, but derived serialization is not itself a
proof of HQ canonicalization. Canonical encoding, duplicate-key handling, numeric rules, field
ordering where required, size measurement after escaping, and exact received-byte retention need
purpose-built tests and in some places handwritten decoding.

The Rust Nostr SDK documents support for NIP-44 and NIP-59, making it a strong candidate for the
standard protocol primitives. HQ should nevertheless own and test its application-level envelope,
limits, identity binding, and error classification. A crate accepting an input does not prove that
HQ should accept it.

This area needs byte-level fixture vectors generated independently where possible, including valid,
invalid, boundary-sized, non-canonical, duplicate, tampered, and cross-language cases.

### 2. The causal reducer and authorization model

This is the highest semantic risk. The reducer handles missing parents, multiple event statuses,
mailbox capabilities, revocation, observations, human-account membership, project chains, message
state, activity coalescing, and deterministic conversation ordering. It must converge across valid
arrival orders, and the incremental affected-set path must equal whole-log reduction.

Rust will make individual decisions clearer, but translating existing control flow can still
translate an existing bug. Before using Go as an oracle, known disputed behavior must be classified
as one of:

1. required behavior to preserve;
2. a Go defect that the oracle must not bless; or
3. an intentional clean-break change recorded in a divergence manifest.

The reducer should be ported from the written rules and conformance vectors, not mechanically from
`internal/eventstate/reducer.go`.

### 3. SQLite and the transactional write path

The store is the largest subsystem. It combines append-only canonical data, dependency indexes,
rebuildable projections, operational tables, mutation receipts, change revisions, outbox rows,
quarantine, project workflows, and repair. The order of operations is part of correctness.

Rust provides useful safety here: RAII transactions roll back by default, and `rusqlite` uses a
mutable connection borrow to prevent accidental nested transactions at compile time. It does not
verify SQL semantics, projection ownership, or crash consistency.

For HQ's current one-owner/one-connection design, the simplest fit is likely:

- `rusqlite`, without an ORM;
- one dedicated synchronous store thread that owns the connection;
- typed requests and one-shot responses between Tokio tasks and that thread; and
- transaction-scoped functions that accept a transaction reference and cannot escape it.

This is preferable to making every store operation async merely because the rest of the node uses
Tokio. A `rusqlite::Transaction` is deliberately not `Send` or `Sync`, which reinforces keeping the
entire write unit on its owner thread. SQLx would be worth reconsidering only if HQ changes to a
genuinely concurrent database access model or values compile-time query metadata enough to accept
the extra async/pool machinery.

A new schema is welcome, but it should preserve the conceptual separation among:

- canonical facts;
- deterministic, disposable projections;
- deterministic dependency indexes;
- durable local reconciliation state; and
- ephemeral operational state.

Mixing those categories would make verification and future rebuilds harder even if the new schema
is smaller.

### 4. Node, relay, and subscription lifecycle

The Go implementation relies heavily on contexts and goroutines. Tokio provides typed channels,
`select!`, task tracking, and cancellation tokens, but cancellation remains cooperative. Dropping a
future can interrupt it at an `.await`, so code must make explicit which operations are
cancellation-safe and which must finish once started.

The difficult cases include:

- shutdown while a database mutation is in progress;
- reconnect while a mutation response is lost;
- registering a subscription without losing the revision race;
- draining accepted harness output while rejecting new intake;
- relay disconnect during exact-wrapper publication;
- node restart while clients hold old sockets; and
- child termination escalation without orphaning a process or losing durable output.

Rust can prevent data races and non-`Send` state from crossing tasks, but it cannot infer these
protocols. Each long-lived component needs an explicit lifecycle enum, a single owner, bounded
mailboxes, and a shutdown contract. Tokio's own guidance describes graceful shutdown as three
separate jobs: detect it, notify every task, and wait for completion. HQ should encode all three,
not treat task abortion as normal shutdown.

### 5. Codex bridge and harness supervision

This subsystem parses an evolving provider JSON-RPC protocol, correlates provider operations with
HQ messages and activity, reads stdout/stderr, applies backpressure and coalescing, supervises
processes, maintains leases, and protects environment data.

Rust's enums and ownership are a major improvement for provider notifications and supervisor
states. The hard parts remain partial I/O, malformed provider messages, out-of-order responses,
shutdown races, bounded buffering, and “persist output before activity” reconciliation.

The fake harness and reusable adapter conformance suite are valuable assets. Their behavior should
be moved into language-neutral transcript fixtures early, while keeping richer Rust-only model
tests for lifecycle transitions.

### 6. Ratatui is a rendering layer, not Bubble Tea in Rust

Ratatui is an immediate-mode renderer. Its documentation explicitly leaves event handling,
application state, redraw scheduling, and large-application structure to the program. Crossterm is
the likely input/backend layer. Therefore a direct translation of Bubble Tea commands and messages
would be an awkward design.

The Rust TUI should instead have:

- a pure `UiModel`;
- a closed `UiEvent` enum for keys, mouse, resize, tick, RPC invalidation, reconnect, and command
  completion;
- a pure or nearly pure `update(model, event) -> Vec<UiEffect>` transition;
- a renderer that only borrows the model;
- one effect executor for RPC and timers; and
- explicit restoration of terminal mode on every exit and panic path.

Draft persistence, selection anchors, focus, modal state, scrolling across mixed message/activity
entries, reconnect, and stale-response suppression are more important verification targets than
matching the old screen cell for cell.

Ratatui may make custom layouts and deterministic rendering tests pleasant, but it will not
automatically replace Bubbles' text areas, viewport behavior, Markdown presentation, or Bubble
Tea's command loop. Those either require carefully selected widgets or HQ-owned components.

### 7. CLI/RPC startup and diagnostics

The clean break allows a much smaller local protocol, but all existing recovery semantics still
need a home: version negotiation, autostart ownership, readiness, stale socket cleanup, mutation
retry, reconnect, resubscribe, and typed incompatibility errors.

Rust should use structured error enums at library boundaries and add human context only at binary
boundaries. `thiserror`-style domain errors plus an application context/reporting layer would let
the CLI render a short primary failure followed by actionable causes, paths, and suggested checks.
Flattening all failures into strings would lose one of the port's strongest benefits.

The node-readiness failure that motivated recent work is a useful acceptance case: the user should
see which phase failed, the socket and state directory involved, the last child/node error if one
exists, and a next diagnostic action. Debug detail can remain in structured logs.

### 8. Projects and resource reconciliation

Project history is a home-issued linear protocol layered on the causal event system, with command
envelopes, expected heads, resource claims, assignments, worktrees, dispatch records, audits, and
runtime operations. This is a second family of state machines, not just more rows.

Rust enums can eliminate many invalid action/status combinations, but this is also an area where
over-generic abstractions could obscure policy. Prefer explicit transition functions and shared
small traits for true common laws. Do not build one universal state-machine framework merely
because Rust makes it possible.

## Where Rust removes Go verbosity

### Algebraic data types and exhaustive matching

Go commonly represents a closed choice as a string constant plus a struct containing fields that
are meaningful only for certain choices. Rust can make the choice and its data inseparable:

```rust
enum ConversationEntry {
    Message(Message),
    Activity(HarnessActivity),
}

enum ReductionDecision {
    Projected(ProjectedFacts),
    Unresolved { missing: Vec<EventId> },
    Unsupported(UnsupportedReason),
    Invalid(ValidationError),
    Unauthorized(AuthorityError),
}
```

Adding a variant produces compiler errors at unhandled matches. There is no possible activity
entry with a half-populated message payload, and no status string silently falling through a
default branch.

This is probably the single largest maintainability improvement for HQ.

### Newtypes instead of interchangeable strings

HQ has many values that are strings in storage or JSON but are not interchangeable:

```rust
struct EventId([u8; 32]);
struct InstallationId(Uuid);
struct MailboxId(Uuid);
struct HumanAccountId(Uuid);
struct MutationId(Uuid);
struct ProviderId(String);
struct ExternalSessionId(String);
struct RelayUrl(url::Url);
```

Constructors perform validation once. APIs cannot accidentally pass a mailbox ID where an account
ID is expected. Serialization and database conversion live with the type rather than being
repeated at every boundary.

### `Result`, `Option`, `?`, and pattern matching

Rust's `?` removes much repeated `if err != nil` plumbing while preserving typed errors. `Option<T>`
distinguishes absence from a zero value, and matching a tuple of options makes multi-field
invariants visible. It also removes the ambiguous “nil interface containing a typed nil” class of
failure.

This makes validation and conversion pipelines shorter, although rich error context still needs to
be added deliberately.

### Traits as disciplined typeclasses

Rust traits can provide much of the typeclass-like vocabulary that motivated considering this
port: shared behavior, associated types, bounded generic algorithms, default methods, and static
dispatch. They would be useful for concepts such as:

```rust
trait CanonicalEncode {
    type Error;
    fn canonical_bytes(&self) -> Result<Vec<u8>, Self::Error>;
}

trait Projection {
    type State;
    type Fact;
    fn apply(state: &mut Self::State, fact: &Self::Fact);
}
```

Traits do **not** by themselves prove the laws we care about. Associativity, idempotence,
convergence, incremental/batch equivalence, and authorization monotonicity still require property
tests or formal modeling. Rust also lacks Haskell-style higher-kinded types and makes some highly
generic functional patterns cumbersome. The best use of traits here is to name stable boundaries
and laws, not to recreate a category-theory library inside HQ.

### Validated wrappers and typestate

Separate types can make trust transitions explicit:

```rust
struct RawWireEvent(Vec<u8>);
struct ParsedEvent { /* untrusted fields */ }
struct VerifiedEvent { /* ID and signature verified */ }
struct AuthorizedEvent { /* authorization decision and dependencies */ }
```

Only a `VerifiedEvent` can enter canonical storage, and only reduction can produce projected facts.
This is more robust than comments saying that a shared struct is “already validated.” Full generic
typestate with `PhantomData` is probably unnecessary; ordinary wrapper types are clearer at HQ's
I/O boundaries.

### Ownership, borrowing, and RAII

Ownership makes unintended shared mutable state harder to create. Immutable borrows fit the pure
reducer. RAII can reliably restore terminal modes, release locks, roll back transactions, close
files, and clean up temporary resources on early return.

The tradeoff is that a graph reducer containing several interdependent maps can fight the borrow
checker if written as in-place mutation everywhere. Favor immutable input facts, stable IDs,
short-lived borrows, and explicit result values rather than reaching for pervasive `Arc<Mutex<_>>`.

### Iterator and conversion vocabulary

Iterators, `collect::<Result<_, _>>()`, `From`/`TryFrom`, and destructuring remove many index loops,
temporary slices, conversion helpers, and manual accumulator/error patterns. These wins are real
but secondary to better domain types.

### Compile-time concurrency boundaries

`Send` and `Sync` requirements catch accidental cross-task sharing. A dedicated store owner, relay
actor, supervisor actor, and TUI event loop can each expose typed commands rather than locks over
their internals. This reinforces HQ's existing single-owner architecture.

### Tooling and test expressiveness

Rust offers a strong default toolchain (`cargo`, rustfmt, Clippy, documentation tests) and good
libraries for the kinds of verification HQ needs:

- Proptest can generate and shrink causal DAGs and state-machine command sequences.
- Snapshot testing can cover normalized domain views, errors, and Ratatui buffers.
- Loom can exhaustively explore small synchronization components when they use Loom-visible
  primitives. It cannot model SQLite, child processes, or a whole Tokio application and should not
  be oversold.

## Where Rust does not remove verbosity

Some parts will stay verbose or become more explicit:

- SQL schemas, joins, and transaction ordering.
- Exact protocol validation and useful rejection diagnostics.
- Custom Serde code for canonical or adversarial JSON.
- Tokio cancellation, task ownership, and shutdown orchestration.
- Ratatui input, widget state, redraw scheduling, and Markdown/text-area behavior.
- Error conversions across crate boundaries.
- Lifetime and ownership annotations around streaming or borrowed data.
- Generic trait bounds when abstractions become too ambitious.

Go remains better at quick compilation, simple process/network code, low-ceremony interfaces, and
easy debugging of straightforward imperative control flow. A Rust rewrite should not be judged by
whether every file is shorter. It should be judged by whether invalid states and unsafe transitions
become harder to express.

## Proposed Rust architecture

The following is a dependency sketch, not a mandate to maximize the number of crates:

```text
crates/
  hq-types       IDs, bounded values, commands, domain enums
  hq-wire        strict parsing, canonical encoding, signatures, envelopes
  hq-reducer     pure causal graph, authority, ordering, reduced state
  hq-store       SQLite schema, transactions, indexes, projections, repair
  hq-rpc         versioned local protocol and reconnect semantics
  hq-transport   relay sessions, wrapping, retry, staging, quarantine
  hq-harness     provider-neutral contract and conformance types
  hq-codex       Codex app-server adapter
  hq-node        composition root, actors, lifecycle, subscriptions
  hq-tui         UiModel, UiEvent, UiEffect, Ratatui renderer
  hq-cli         command parsing and user-facing reports
  hq-testkit     fixtures, scripted relay/provider, normalized snapshots
```

Small crates are useful only when they enforce dependency direction. The critical rule is that
`hq-reducer` cannot import SQLite, Tokio, Ratatui, relay clients, wall clocks, random number
generators, or process APIs. The node and adapters depend inward on the domain core, never the other
way around.

### Functional core

The functional core should expose explicit transformations:

```rust
fn parse_and_verify(raw: RawWireEvent) -> Result<VerifiedEvent, WireError>;

fn reduce<'a>(
    events: impl IntoIterator<Item = &'a VerifiedEvent>,
    policy: &ReductionPolicy,
) -> ReducedState;

fn decide_command(
    state: &DomainSnapshot,
    command: DomainCommand,
) -> Result<UnsignedEventPlan, CommandError>;

fn apply_ui_event(model: UiModel, event: UiEvent) -> (UiModel, Vec<UiEffect>);
```

The imperative shell supplies time and randomness, signs event plans, runs transactions, executes
effects, sends RPC, and renders frames. Time, randomness, filesystem paths, environment variables,
and network responses should enter as explicit values at the core boundary.

Do not force every subsystem into one abstraction. The causal reducer, project transition logic,
supervisor, and UI are different state machines with some shared vocabulary but different laws.

### Runtime ownership

A sensible operational model is:

- one Tokio runtime for sockets, relays, timers, RPC clients, and process pipes;
- one dedicated SQLite owner thread;
- one actor/task per relay session;
- one supervisor owner with explicit child records;
- one bounded channel at every potentially unbounded producer; and
- one cancellation tree rooted at node lifetime, with tracked graceful completion.

The binaries should be thin composition roots. Library crates return typed errors; the CLI/TUI
choose concise user wording and attach diagnostic reports.

### Likely foundation crates

These are candidates to validate with a small spike, not irrevocable commitments:

- Ratatui plus Crossterm for rendering and terminal events.
- Tokio and tokio-util for async I/O, cancellation, and task tracking.
- `rusqlite` for the single-owner SQLite store.
- Serde/serde_json, with handwritten strict code where canonical rules demand it.
- rust-nostr primitives or SDK components for NIP behavior, wrapped by HQ-owned types.
- `tracing` for structured diagnostics and correlation spans.
- a typed library-error crate pattern plus an application reporting layer.
- Proptest, snapshot tests, and narrowly scoped Loom models.

Do not choose an ORM, async trait framework, dependency-injection framework, or universal actor
framework until a concrete use demonstrates that it reduces rather than relocates complexity.

## Verification strategy

Verification should be a deliverable that precedes and shapes the rewrite. “The new TUI seems to
work” is the final smoke test, not the proof of switchover.

### 1. Freeze intended semantics, not every Go behavior

Create a versioned behavior ledger before implementing Rust. Each contract is categorized as:

- **preserve**: required domain or safety behavior;
- **replace**: intentional clean-break behavior with a documented Rust expectation; or
- **discard**: old implementation detail with no externally meaningful contract.

Known bugs must be resolved or entered as explicit expected divergences. Otherwise differential
testing will pressure the Rust implementation to reproduce them.

The primary specification is the design/event/project documentation plus reviewed examples. Go is
an executable oracle where it agrees with that specification, not an unquestionable source of
truth.

### 2. Build a language-neutral conformance runner

Both implementations should accept JSON Lines test operations and emit a normalized JSON Lines
result. This is test-only infrastructure, so it does not constrain the production RPC protocol.

Useful operations include:

- parse/verify a raw canonical event;
- create a deterministic signed event using supplied key/time/randomness;
- reduce a supplied event set;
- ingest events in a supplied arrival schedule;
- issue a domain command with a fixed mutation ID;
- snapshot selected domain views;
- close/reopen/repair a store;
- run a scripted relay transcript;
- run a scripted provider process transcript; and
- apply a sequence of abstract TUI events and return normalized model/render state.

Normalized output should compare domain meaning, not storage artifacts. Stable IDs, statuses,
causal reasons, ordering, public message/activity shapes, outbox intent, receipts, revisions, and
error categories belong in it. SQLite row IDs, table layouts, socket bytes, log prose, and screen
colors generally do not.

### 3. Partition exact compatibility from semantic compatibility

The suite must state what kind of equality each case expects:

| Surface | Equality required |
| --- | --- |
| Standard crypto vectors | Exact bytes/IDs/signatures as defined by the relevant NIP/BIP |
| New Rust canonical schema | Exact Rust fixture bytes; no requirement to match Go schema 3 |
| Reducer and authorization | Same reviewed semantic decision unless listed as a divergence |
| Incremental versus full reduction | Exact normalized equality within Rust |
| SQLite | Same observable state and crash guarantees; no table/schema equality |
| Local RPC | Same retry/reconnect intent; no wire equality |
| Relay behavior | Same delivery/deduplication guarantees; exact wrapper equality only within one retry lineage |
| Harness | Same provider-neutral accepted behavior and persistence ordering |
| TUI | Same user-visible state transitions; no pixel or keybinding compatibility requirement |
| Errors | Same category and actionable data; wording may improve |

This partition prevents “no backwards compatibility” from accidentally discarding invariants, and
prevents the oracle from freezing unimportant encodings.

### 4. Preserve and expand deterministic fixture corpora

Before retiring Go, export test fixtures for:

- every event type, scope, reduction status, and authorization failure;
- missing, late, duplicate, concurrent, and unusable parents;
- grants, observations, revokes, regrants, device acceptance, and revocation races;
- conversation order ties and activity coalescing;
- project success, stale head, fork, resource conflict, and command replay;
- exact canonical and wrapper size boundaries;
- duplicate JSON keys, unknown fields, malformed UTF-8/escapes, invalid IDs/signatures, and wrong
  audience/key bindings;
- lost mutation responses and mutation-ID conflicts;
- relay reconnect, duplicate delivery, staging, quarantine, and wrapper reuse;
- provider partial frames, unknown notifications, out-of-order responses, buffer saturation, and
  shutdown; and
- TUI draft/focus/selection survival across invalidation and reconnect.

Deterministic tests inject keys, clocks, random bytes, relay responses, and process transcripts.
Tests that depend on sleeps or real public relays should be treated as smoke tests only.

### 5. Add properties that do not depend on Go

Differential testing catches divergence; property testing checks deeper laws. Generate causal DAGs
and command sequences and assert:

- reduction is idempotent;
- every valid topological arrival order converges;
- duplicate ingestion changes no semantic state;
- incremental projection equals a whole-log rebuild;
- projected events never depend on unresolved, invalid, unsupported, or unauthorized facts;
- conversation order always places parents before children and is otherwise deterministic;
- a later unrelated event cannot retroactively authorize an earlier unauthorized action;
- receipt replay returns the original result, while same-ID/different-input conflicts;
- a committed canonical mutation has its required projections, outbox intent, receipt, and revision
  or has none of them;
- repair is idempotent and leaves operational facts in their documented category;
- accepted harness work is either drained durably or reported as failed, never silently dropped;
- cancellation does not leave terminal mode, locks, sockets, transactions, or child processes in an
  owned-but-untracked state; and
- bounded queues remain bounded under adversarial producers.

Proptest's state-machine support is particularly well suited to database APIs and client/server
interactions because it can shrink a failing transition sequence to a small reproducible case.

### 6. Test crash boundaries, not just returned errors

Inject process termination or failpoints immediately before and after:

- canonical insert;
- projection/index updates;
- outbox derivation;
- mutation receipt write;
- revision allocation;
- transaction commit;
- gift-wrap persistence;
- relay acceptance persistence;
- provider output persistence;
- activity persistence; and
- child shutdown/drain completion.

After every injected crash, reopen and run repair. Assert a valid old state or valid new state, never
a hybrid. SQLite transaction rollback covers part of this, but the provider, relay, and child
process reconciliation windows cross transaction boundaries and need explicit tests.

### 7. Differential shadow runs must be isolated

CI can run the Go and Rust conformance binaries against the same generated semantic trace, each
with its own temporary state and scripted relay/provider. Compare normalized snapshots after every
operation and after reopen/repair.

Do not point both implementations at one database, one Unix socket, one live provider child, or one
live installation key. Dual-running the same identity against public relays could create misleading
duplicates and new causal facts rather than a controlled comparison.

Sanitized existing event logs can still be valuable as *test inputs* even though the Rust product
does not promise to import them. A converter owned by the test harness can translate old facts to
the new schema, with every translation recorded. That exercises realistic graph size and shape
without creating a migration commitment.

### 8. Performance and operability gates

Record Go baselines and set Rust acceptance thresholds for:

- cold node readiness and client autostart;
- full rebuild at representative event counts;
- incremental ingestion of late-parent and high-fanout account events;
- TUI first paint and invalidation-to-redraw latency;
- idle and active memory;
- relay/provider queue behavior under backpressure;
- binary size and release build time; and
- shutdown/drain time.

The goal need not be “Rust is faster everywhere.” The goal is to detect accidental algorithmic
regressions and ensure that stronger types did not hide expensive cloning or serialization.

### 9. Cutover procedure

Because this is a clean break, prefer a reversible operational cutover over an in-place migration:

1. Freeze feature development in Go except oracle fixes and critical defects.
2. Finish the conformance runner, fixture corpus, property suite, and divergence ledger.
3. Reach subsystem parity in the order: wire/reducer, store/repair, node/RPC, relay, harness,
   CLI/TUI.
4. Run differential CI and repeated crash/reopen suites until no unexplained divergence remains.
5. Dogfood Rust with a new identity and new state directory against controlled peers/relays.
6. Archive the Go database, key, binary version, and logs read-only. Do not let Rust mutate them.
7. Start Rust as the sole active node. Never have both nodes publish as the same identity.
8. Keep launching the archived Go binary against its archived state as the rollback path, not as a
   converter for Rust state.
9. After a defined soak period and successful recovery drills, remove Go from normal builds while
   retaining fixtures and the oracle binary/source long enough to diagnose regressions.

If continuity of existing conversations becomes desirable, design one explicit, auditable import
command. It should read an export, validate every fact, write only the new database, produce a
conversion report, and never make the Rust node understand the Go database during normal use.

### 10. Switchover acceptance matrix

The Rust implementation is ready only when all rows have evidence:

| Area | Required evidence |
| --- | --- |
| Wire/crypto | Standard vectors, adversarial corpus, exact retry-wrapper tests |
| Reducer | Reviewed fixtures, DAG property tests, convergence, deterministic order |
| Authorization | Grant/revoke/account race matrix and generated causal cases |
| Store | Incremental/rebuild equality, mutation atomicity, failpoint crash matrix |
| RPC/node | Autostart, version, reconnect, retry, revision-race, graceful restart tests |
| Relay | Scripted disconnect/duplicate/retry/staging/quarantine tests and controlled integration |
| Harness | Provider transcript conformance, backpressure, persistence ordering, drain/kill tests |
| Projects | Transition-model tests, stale-head/conflict/reconciliation fixtures |
| TUI | Model transitions, render snapshots, terminal restoration, reconnect/draft scenarios |
| Diagnostics | Stable error categories, actionable startup failures, secret-redaction tests |
| Operations | Performance baselines, soak, clean shutdown, recovery drill |

No row should be waived merely because the old binary is pre-release. Pre-release status permits a
clean break in representation; it does not reduce the cost of losing or misauthorizing signed
state.

## Suggested implementation order

The sequence should retire uncertainty early rather than maximize visible UI progress:

1. **Specification and oracle.** Review semantics, classify known bugs, create the conformance
   protocol, extract fixtures, and establish Go baselines.
2. **Types and wire spike.** Validate Serde strictness, Nostr primitives, signing vectors, error
   modeling, and canonical byte ownership.
3. **Pure reducer.** Implement the full batch oracle first with property tests; optimize an
   incremental affected-set engine only after the oracle is stable.
4. **SQLite store.** Introduce the dedicated owner and new schema; require repair equality from the
   first durable mutation.
5. **Node and local RPC.** Build typed lifecycle, autostart/readiness, mutation retry, and
   subscriptions around fake transport/harness ports.
6. **Relay transport.** Add live Nostr behavior after scripted transport is exhaustive.
7. **Harness and Codex adapter.** Port the provider-neutral contract before Codex-specific
   translation and process supervision.
8. **CLI and Ratatui.** Drive both from the same domain client; keep UI update logic independent of
   RPC effects.
9. **System verification and cutover.** Differential runs, crash injection, performance, dogfood,
   archive, and sole-owner launch.

The batch reducer first is important. It gives the new store its own offline truth oracle and
prevents incremental optimization from becoming the only implementation of the rules it is meant
to optimize.

## Pros and cons for future iteration

| Dimension | Rust | Go |
| --- | --- | --- |
| Domain modeling | Strong advantage: enums, newtypes, exhaustive matches | Conventions and tests carry more of the burden |
| Functional core | Strong advantage: immutable borrowing and explicit results | Possible, but interfaces/structs express fewer invariants |
| Protocol refactoring | Safer compiler-guided changes | Faster edits, greater risk of missed switch/zero-value paths |
| Reducer correctness | Better representation and property-test ecosystem | Simpler language, but more invalid states remain representable |
| SQLite transactions | RAII and ownership help; SQL remains verbose | Mature and straightforward; current code already works |
| Async lifecycle | Strong race safety, explicit ownership; cancellation remains subtle | Goroutines/contexts are concise and familiar |
| TUI | Flexible deterministic rendering; architecture is HQ's responsibility | Charm stack provides a cohesive update/command ecosystem |
| Provider/JSON integration | Tagged enums improve known messages; strictness is controllable | Quick handling of loose JSON and evolving protocols |
| Errors | Rich typed errors and chains | Easy wrapping, but categories often degrade to strings/sentinels |
| Compile/feedback loop | Slower builds and more compiler negotiation | Faster builds and less ceremony |
| Operational binary | No GC, predictable ownership, good static binary story | Excellent simple deployment and cross-compilation |
| Team onboarding | Higher language/concurrency learning curve | Lower barrier and smaller language |
| Large refactors | Compiler acts as a broad checklist | Tests/search must find more semantic fallout |

My expected iteration profile after parity is:

- **Clearly easier in Rust:** adding event variants, changing authorization/reducer state,
  separating trusted/untrusted representations, refactoring domain APIs, and testing generated
  state-machine cases.
- **Modestly easier:** store transactions, structured diagnostics, RPC request/response modeling,
  provider event modeling.
- **Mixed:** supervisor/relay concurrency. Safer does not always mean shorter or faster to write.
- **Potentially harder:** quick TUI experiments, loose integration with changing JSON, compile-time
  feedback, and abstractions that trigger complex lifetime/generic errors.

## Recommendation

Proceed with a Rust replacement if the project intends to keep the signed event model, causal
authorization, offline rebuild oracle, and increasingly rich project/harness state machines. Those
are exactly the areas where stronger domain types and compiler-checked boundaries repay their
cost.

Do not proceed merely to obtain a nicer TUI or fewer lines. Ratatui will not supply Bubble Tea's
architecture, and the store/relay/supervisor complexity will remain.

The first milestone should not be a Rust binary. It should be the reviewed behavioral ledger,
language-neutral conformance runner, adversarial fixture corpus, and reducer laws. Once those exist,
the rewrite is a controlled replacement with measurable evidence. Without them, a clean-break port
would be an attractive new implementation whose most important semantic regressions might remain
invisible until real signed state exercises them.

## Current primary references

- [Ratatui rendering and application-architecture responsibilities](https://ratatui.rs/concepts/rendering/)
- [Ratatui event-handling approaches](https://ratatui.rs/concepts/event-handling/)
- [Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown)
- [rusqlite transactions and rollback-on-drop](https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html)
- [Serde enum representations](https://serde.rs/enum-representations.html)
- [Serde strict unknown-field handling](https://serde.rs/container-attrs.html)
- [Rust Nostr SDK NIP support](https://rust-nostr.org/sdk/nips/index.html)
- [Proptest state-machine testing](https://proptest-rs.github.io/proptest/proptest/state-machine.html)
- [Loom concurrency model testing and limitations](https://docs.rs/loom/latest/loom/)
