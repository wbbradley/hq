# HQ Rust rewrite: top-down architecture and implementation plan

Status: governing architecture and implementation plan accepted for autonomous execution on
2026-08-26.

## Decision

HQ will be reimplemented in Rust as a new system that preserves the product's distributed model
and causal fact algebra, but not the Go implementation's representations or interfaces.

This is a clean-sheet rewrite, not a port in the usual sense:

- The Go tree is frozen and becomes historical reference material.
- Rust behavior is derived from reviewed requirements and algebraic laws, not Go control flow.
- The Rust system gets a new canonical protocol version, database, local RPC protocol, state
  directory, and implementation structure.
- A Rust node will not read a Go database, decode old HQ canonical events, share a state directory
  with Go, or promise CLI, TUI, RPC, schema, or file-format compatibility.
- Go/Rust differential equality is not a release criterion. Go tests and logs may contribute
  scenarios, but they are not an oracle when the new specification differs or when the old
  behavior is defective.
- There will be no line-by-line or package-by-package translation. Rust modules are chosen around
  responsibilities, dependency direction, and invariants.

The enduring model is:

> HQ is a local authority and runtime coordinator built on an add-only set of signed causal facts.
> Replicas merge facts by set union. Pure deterministic reduction derives views that may retract
> as new facts revoke, archive, reject, or dominate old facts.

This document defines the overall shape and construction order. It intentionally does not define
individual wire fields, SQL tables, RPC methods, or UI keys. Those belong in the specifications
identified below and must be internally reviewed against this architecture before implementation.

## Autonomous execution and cutover boundary

The rewrite is intended to be executable as one persistent Codex goal using `PLAN.md` as the full
ordered queue of remaining work. The goal agent may split or refine queued work, add newly
discovered required work, and resolve implementation choices without waiting for routine approval.
For a choice not fixed by this document, it should select the simplest first-principles design
consistent with the retained product intent, security model, and non-goals, then record material
reasoning in an ADR.

The agent must not narrow the final outcome, silently discard a retained capability, introduce Go
compatibility, or make a materially different product or security decision merely to avoid a
question. It should stop only when a decision would expand authority, alter the agreed product
boundary, require destructive or externally consequential action, or cannot be resolved from the
documented principles and evidence.

The autonomous rewrite goal ends with a verified, cutover-ready Rust release candidate and an
operator-reviewed cutover procedure. Archiving, disabling, replacing, or deleting a live Go
installation; activating a production identity; and declaring the soak period successful remain
explicit operator actions outside that goal unless separately authorized.

## Requirements and source precedence

The rewrite needs a single place to resolve conflicts among the current sources. The order is:

1. The explicit rewrite decisions and non-goals in this document.
2. The reviewed algebraic laws in `../crdt-algebra-laws.html`.
3. New Rust-era domain and protocol specifications written as part of this plan.
4. Product intent in `docs/`, after removing Go versions, legacy affordances, and implementation
   details.
5. Go tests and code as examples of scenarios, failure modes, and prior design choices.

`docs/events.md`, `docs/design.md`, `docs/nostr.md`, `docs/harnesses.md`, and
`docs/projects.md` are therefore inputs, not automatically normative Rust specifications. In
particular, references to canonical schema 3, SQLite schema 33, local wire 7, Go packages, the Go
Nostr library, Bubble Tea behavior, and a particular historical Codex version do not carry into
the new design merely because they exist today.

`rust-port.md` remains the companion feasibility, risk, and verification analysis.
`rust-port-transcript.md` records the decision history. Where either suggests using Go as a
behavioral oracle, this document's clean-sheet source precedence controls instead.

Before implementation, every externally meaningful requirement is entered in a behavior ledger
as one of:

- **retain**: a product, safety, security, or algebraic property the Rust system must provide;
- **redesign**: a desired capability whose Rust-era semantics will be specified afresh; or
- **drop**: a Go feature or behavior that is intentionally absent.

Unknown behavior is not silently retained. A disputed requirement blocks only its dependent
feature, not unrelated foundation work.

## Product boundary

### Retained capabilities

The intended first complete Rust release includes these product responsibilities:

- one local node per installation, owning identity use, durable state, reduction, remote
  synchronization, subscriptions, and managed agent runtimes;
- installation identities, mailboxes, peers, directional mailbox capabilities, and multi-device
  human accounts;
- questions, answers, asynchronous messages, cancellation, archive/restore/reject state, and
  deterministic conversation ordering;
- signed harness activity as a separate non-actionable stream in conversations;
- encrypted Nostr-based replication through retained relays, including offline outbox and catch-up;
- named agents, durable provider sessions, process supervision, delivery reconciliation, and a
  provider-neutral harness contract with a Codex adapter;
- projects, path-resource claims, assignments, threads, remote project commands, and explicit
  sagas for filesystem, Git, and runtime side effects;
- a reconnecting local client API used by both CLI and Ratatui clients; and
- repair, diagnostics, auditability, bounded queues, and graceful shutdown.

Each item remains subject to the new specifications. Retaining a capability does not retain its Go
command spelling, JSON shape, table layout, timing constant, or screen layout.

### Non-goals

The rewrite will not provide:

- Go database migration, Go event decoding, Go local-RPC compatibility, or mixed-version clusters;
- concurrent Go and Rust nodes using one installation identity;
- source-level similarity to Go packages or preservation of Go abstractions;
- a general-purpose CRDT framework or universal state-machine framework;
- public messaging, multi-writer project authority, project re-homing, or remote project execution
  unless separately added to the Rust-era product scope;
- a security boundary against other processes running as the same operating-system user; or
- proof that an external process has stopped touching a resource merely because HQ released its
  advisory claim.

If import of historical content is ever desired, it will be a separate, explicit, auditable
offline tool. The normal Rust node will remain unaware of Go storage and schemas.

## System model and invariants

### Deployment model

```text
                         retained Nostr relays
                    +-----------------------------+
                    | encrypted immutable facts   |
                    | and remote control envelopes|
                    +-------------+---------------+
                                  |
              +-------------------+-------------------+
              |                                       |
      +-------v---------+                     +-------v---------+
      | installation A  |                     | installation B  |
      | one Rust node   |                     | one Rust node   |
      | one identity    |                     | one identity    |
      | one database    |                     | one database    |
      +---+----------+--+                     +---+----------+--+
          |          |                            |          |
      local API   managed                     local API   managed
      clients     runtimes                    clients     runtimes
```

Relays transport encrypted data; they are neither application authorities nor sources of truth.
The local socket is a transport optimization and ownership boundary, not a different domain model.
Clients never sign facts, open SQLite, own relay sessions, or supervise provider processes.

### Algebraic contract

Let `E` be the set of verified canonical facts, keyed by canonical fact ID, and `R(E)` the complete
reduction result. The implementation must preserve all nine laws from the algebra note:

1. Merge is set union and is commutative, associative, idempotent, and has the empty set as identity.
2. Reduction is invariant under input order and duplication.
3. Incremental reduction is observationally equal to complete batch reduction.
4. Causal relationships, not receipt or wall-clock order, determine dominance.
5. A frontier contains every and only causally maximal fact in its aggregate.
6. Missing dependencies defer a fact; their arrival reconsiders its dependent closure.
7. Authority is explicit, historical, and causal.
8. Facts are monotone while materialized projections may retract.
9. Every concurrent conflict has an explicit deterministic domain rule.

The batch reducer is the executable definition of `R`. Incremental reduction is an optimization
that must continually prove equality with it. There is one canonical comparator for presentation
order; storage queries and UIs consume its result or an index derived from it and may not implement
lookalike ordering rules.

### System-wide invariants

- Canonical facts are immutable and deduplicated by a content-derived, signed identity.
- Exact protocol bytes are stable and testable within the new protocol; this is required for
  signatures, Rust-to-Rust interoperability, audit, and exact retry, not for Go compatibility.
- Raw input is untrusted. Parsing, cryptographic verification, graph classification, and domain
  projection are distinct transitions represented by distinct types.
- A fact that depends on absent or unusable parents never changes a projection prematurely.
- Authorization uses explicitly cited authority facts at the action's causal point. Current
  projection state is not a substitute for historical authorization.
- Human-device membership tracks all causal maxima. Revoke/regrant/reaccept must use the correct
  post-revoke acceptance frontier; no lexicographically selected historical acceptance may stand
  in for the frontier.
- Locally authored and remotely received canonical facts enter the same commit and reduction path.
- Every fact-backed local mutation is atomic with its canonical append, dependency data,
  projections, outbox intent, idempotency receipt, and change revision.
- Database state is not evidence that an external filesystem, network, Git, or provider operation
  occurred. Cross-boundary work uses durable, idempotent sagas with explicit uncertain states.
- All retryable commands and external submissions have stable identities. Reusing an identity for
  different input is a conflict.
- Potentially unbounded producers terminate at bounded queues with an explicit backpressure,
  coalescing, or rejection policy.
- Shutdown stops intake, signals owned tasks, drains accepted durable work, escalates external
  process termination when needed, and waits for owned lifetimes.
- Secrets and caller environments never enter signed facts, projections, receipts, protocol
  diagnostics, or ordinary structured logs. Provider stderr has its own documented local trust
  boundary.

## Architectural style

HQ uses a functional core with an imperative shell:

- **Domain values and policies** are typed and pure.
- **Causal reduction** is deterministic and free of I/O, clocks, randomness, processes, and
  runtime dependencies.
- **Application services** coordinate use cases through narrow ports.
- **Adapters** own SQLite, Nostr, local IPC, terminals, filesystems, Git, and provider protocols.
- **The node** is the only composition root and runtime owner.

Traits name real substitutable boundaries or test seams. They do not abstract every data type and
do not claim algebraic laws that only tests can establish. Prefer enums, newtypes, ordinary
functions, and concrete collections inside the core. Prefer message ownership to shared mutable
state in the shell.

Time, random bytes, generated IDs, environment snapshots, and filesystem observations enter pure
logic as explicit values. Library code returns typed errors; only the binary and UI layers turn
them into prose.

## Component boundaries

### Domain model

The domain component owns validated values and semantic choices:

- newtyped IDs, keys, addresses, timestamps, sequence numbers, resource locators, and bounded text;
- fact payload enums, audiences, causal references, and authority references;
- commands, command outcomes, normalized queries, and public view models;
- messages, activities, accounts, capabilities, named agents, sessions, and projects; and
- typed error categories and stable diagnostic fields.

It does not own JSON field names, SQL rows, sockets, tasks, terminal widgets, relay frames, or
Codex messages. Protocol DTOs convert into domain types with `TryFrom`; protocol serialization is
never derived accidentally from an internal domain struct.

### Causal reducer

The reducer owns:

- the causal graph and reachability relation;
- dependency and reverse-dependency semantics;
- fact classification (`projected`, `unresolved`, `unsupported`, `invalid`, `unauthorized`, or the
  revised Rust-era equivalents);
- causal maxima/frontiers;
- authority evaluation for peers, mailbox capabilities, and human accounts;
- domain reducers for conversations, activity, agents, and projects;
- the one conversation-order comparator;
- a complete batch entry point; and
- a pure affected-closure calculation for incremental callers.

Its input is a set of cryptographically verified semantic facts plus an explicit reduction policy.
Its output is a `ReductionReport` containing decisions, dependency information, reduced aggregates,
and projection facts. It does not know whether input came from SQLite, a relay, or a fixture.

The reducer should be internally split by domain rules, not implemented as one giant match. Shared
causal machinery remains common; capability, account, conversation, activity, agent, and project
policies remain explicit modules with explicit conflict laws.

### Canonical protocol and cryptography

The canonical protocol component owns the new application protocol version:

- strict wire DTOs and their version dispatch;
- deterministic canonical encoding;
- size, count, duplicate-field, and text validation;
- NIP-01 event identity and BIP-340 signature verification/signing;
- exact received-byte retention and content-derived deduplication identity; and
- conversion between verified protocol DTOs and semantic facts.

The trust pipeline is visible in types:

```text
RawCanonicalBytes
  -> ParsedCanonicalEvent
  -> CryptographicallyVerifiedEvent
  -> VerifiedFact
  -> reducer FactDecision
  -> projected domain facts
```

Graph-dependent authorization and missing-parent decisions belong to reduction, not wire parsing.
The protocol specification will be written from scratch as canonical version 1. No compatibility
decoder or schema translation module will be created.

### Persistence and commit engine

The persistence component owns SQLite and the only durable commit boundary. Initially, one
dedicated synchronous thread owns one `rusqlite` connection. Tokio tasks send bounded typed requests
and receive one-shot responses; no transaction or row type crosses that thread boundary.

The commit engine provides three coarse operations rather than exposing table-shaped CRUD:

- execute a local domain mutation;
- ingest verified canonical input with its transport observation; and
- query or repair authoritative state.

A fact-backed mutation performs, in one SQLite transaction:

1. idempotency-receipt lookup and request-digest comparison;
2. command validation against a transaction-consistent domain snapshot;
3. deterministic event planning using supplied clock/ID/random inputs;
4. signing and canonical append;
5. dependency-index maintenance and affected-closure reduction;
6. projection replacement or patching;
7. durable outbox derivation;
8. mutation-result receipt and change-revision allocation; and
9. commit followed by a non-blocking revision invalidation.

Repair discards rebuildable state and recomputes it with the batch reducer. Incremental and repair
outputs must match exactly at the public-query boundary.

Storage is divided by meaning, even if the eventual SQL layout is compact:

| Class | Examples | Rebuildable? |
| --- | --- | ---: |
| Canonical knowledge | exact verified signed facts | no |
| Deterministic indexes | parents, reverse dependencies, frontiers, projection support | yes |
| Materialized projections | conversations, accounts, agents, projects, activity winners | yes |
| Durable operational state | receipts, revisions, outbox wrappers, relay cursors, delivery ledgers, saga checkpoints | generally no |
| Ephemeral runtime state | live sockets, tasks, in-memory environments, UI cache | not stored as domain state |
| Rejected/temporary transport input | bounded quarantine and retry staging | no domain effect |

No client-facing pagination query may scan and sort an entire conversation for every page. Once the
canonical comparator is stable, persistence maintains an index or stable cursor representation
derived from that comparator and proves page concatenation equals reducer order.

### Application services

Application services present use cases independent of transport:

- identity and account workflows;
- mailbox and conversation commands/queries;
- peer, relay-configuration, and synchronization requests;
- agent/session lifecycle requests;
- project commands and activation/close/provisioning sagas; and
- subscriptions and authoritative snapshot refresh.

Ports are declared at this consumer boundary and implemented by the store, relay, harness,
filesystem/Git, and clock adapters. Ports describe capabilities such as `CommitFacts`,
`QueryDomain`, `PublishWake`, `ControlHarness`, and `InspectResource`; they do not mirror every SQL
method.

Application services do not hold long-lived mutable global state. The node owns service instances
and routes completion events back through explicit commands.

### Local client protocol

The local API component owns a new versioned request/response/notification protocol over a
same-user local transport. It provides:

- bounded framing and strict decoding;
- handshake and supported-version negotiation;
- typed mutation IDs, requests, results, and errors;
- lifecycle status/start readiness/stop/restart operations;
- domain queries and mutations;
- revision-based invalidation subscriptions; and
- reconnect, resubscribe, and lost-response semantics in the client library.

An invalidation contains a revision, broad topics, and a full-snapshot flag, never rows or message
bodies. Registration happens before its acknowledged revision is read; activation happens after
the acknowledgement is written so the initial-snapshot race is closed. Per-subscriber wake queues
coalesce to one pending notification and never block commit.

CLI, TUI, and local harness launch clients use this library. None gets a privileged storage path.

### Nostr relay transport

The relay component owns only remote movement and encryption:

- the Rust-era transport envelope version;
- NIP-44 encryption, NIP-59 wrapping, and NIP-42 relay authentication;
- exact wrapper preparation and reuse for one retry lineage;
- one session owner per configured relay;
- retained catch-up, overlapping pagination, live subscription, and deduplication;
- publish attempts, relay acceptance, retry/backoff, staging, and quarantine; and
- conversion of a successfully opened envelope into raw canonical bytes for the common ingest path.

Relay acceptance is not peer receipt and never authorizes a fact. Relay wake signals are coalesced
work notifications; they do not tear down a healthy live subscription. Session recreation is
reserved for connection failure, configuration change, authentication change, or lifecycle
restart.

The relay component does not query projection tables to invent domain behavior and does not apply
facts itself. It requests outbox work and submits inbound bytes through application ports.

### Harness runtime and supervisor

The harness component defines a provider-neutral contract around logical instances, durable
sessions, submissions, interactive requests, normalized output, and normalized activity. It owns:

- capability negotiation and registration checks;
- the accepted/rejected/uncertain submission outcome;
- stable submission identity and lookup/reconciliation requirements;
- bounded durable/coalescing event buffering;
- delivery and output ledgers;
- one logical worker owner per named agent; and
- graceful stop, drain, and ownership release.

The provider-neutral component contains no Codex method names or DTOs. The Codex adapter privately
owns process arguments, JSON-RPC framing, the provider handshake, schema decoding, thread/turn
operations, request handling, and provider-specific diagnostics. Only the node composition root
constructs and registers the adapter.

Provider output becomes a typed event plan and enters the same commit engine as every other local
fact. An accepted buffer item is either durably drained or reported failed; shutdown cannot drop it
silently. Environment snapshots are copied at the control-plane boundary, retained only as
documented in-memory launch templates, and wiped on replacement and shutdown.

### Projects and resources

Pure project transition policy belongs with the domain/reducer. Runtime coordination belongs to
application sagas. The boundary is deliberate:

- the home installation is the sole project mutation and sequencing authority;
- linear project history uses expected-head compare-and-swap;
- desired resources, active claims, assignment epochs, and execution threads are distinct;
- resource conflict and health policy is implemented by a resource-kind adapter, with paths the
  only initial kind;
- database-only transitions commit atomically;
- filesystem, Git, network, and provider effects use stable operation IDs, durable checkpoints,
  reconciliation, and compensation; and
- late output remains attributable history but cannot regain an inactive assignment's authority.

The first implementation should use explicit project transition enums and functions. It should not
introduce a generic workflow engine until at least two genuinely shared workflows demonstrate the
same laws and recovery behavior.

### Ratatui client

The TUI is an ordinary reconnecting local-API client. Its architecture is:

```text
UiModel + UiEvent -> (UiModel, [UiEffect])
                               |
                               v
                    RPC/timer/clipboard executor

UiModel --borrowed only--> Ratatui renderer
```

`UiEvent` includes terminal input, resize, tick, RPC result, invalidation, reconnect state, and
effect completion. The renderer performs no I/O or domain mutation. Effect results carry request
identity so stale completions cannot overwrite newer model state.

Drafts, focus, selection anchors, modal state, and scrolling survive authoritative reloads where
appropriate. Terminal-mode restoration uses RAII and is tested for normal exit, error, and panic.
Screen-cell compatibility with Bubble Tea is irrelevant; semantic state transitions and usable
layout are the contract.

### Node and binaries

The node is the only composition root. It owns:

- the installation lock and identity handle;
- the store thread;
- local API listener and subscriptions;
- relay session actors;
- harness supervisor and provider registry;
- project saga workers; and
- the root cancellation token and task tracker.

The CLI binary is thin: parse input, construct a local client, call one application operation, and
render a typed result. Whether the daemon entry point is a mode of the same executable or a second
binary is a packaging decision, not an architectural boundary.

## Proposed Rust workspace

Crates are used where they enforce dependency direction. Closely related modules may begin in one
crate and split only when the boundary proves useful.

```text
crates/
  hq-domain/
    ids, bounded, facts, commands, views, errors
    identity, capability, account, conversation, activity, agent, project
  hq-reducer/
    graph, reachability, frontier, authority, ordering
    batch, affected, domain reducers
  hq-protocol/
    canonical/v1, strict decoding, encoding, signing, verification
    remote-control/v1
  hq-application/
    use cases, ports, workflows, subscription semantics
  hq-store/
    sqlite actor, schema, commit engine, indexes, projections, repair
    receipts, revisions, outbox, ledgers, staging, quarantine
  hq-local-api/
    protocol/v1, codec, client, server session
  hq-relay/
    envelope/v1, Nostr client, sessions, inbound, outbound
  hq-harness/
    neutral contract, registry, supervisor, delivery, event buffer
  hq-codex/
    private Codex DTOs, JSON-RPC transport, process, adapter
  hq-tui/
    model, event, update, effects, render, terminal guard
  hq-node/
    composition, lifecycle, task ownership, configuration
  hq-testkit/
    builders, keys/clocks, generated DAGs, scripted relay/provider, failpoints
src/bin/
  hq.rs
  hq-node.rs                 # optional packaging choice
```

The intended dependency direction is:

```text
                         hq-domain
                       /     |     \
             hq-reducer  hq-protocol  hq-application (ports/use cases)
                  \          |          /
                   \      hq-store     /
                    \        |        /
        hq-local-api   hq-relay   hq-harness <- hq-codex
                 \         |         /
                  +--------hq-node---------+
                           /   \
                         CLI   hq-tui
```

More precisely, adapters depend inward on domain and application contracts; `hq-node` may depend
on every adapter in order to compose them. `hq-domain` and `hq-reducer` never import Tokio,
SQLite, Nostr, Ratatui, filesystem, process, or provider crates. `hq-harness` never imports
`hq-codex`. Cargo manifests and architecture tests enforce these rules.

## Protocol ownership

The rewrite has several protocols with independent version spaces. They must not share a single
global schema number.

| Protocol | Purpose | Normative specification | Rust owner |
| --- | --- | --- | --- |
| Canonical fact v1 | Signed immutable facts and causal references | `docs/protocol/canonical-v1.md` | `hq-protocol::canonical::v1` |
| Remote control v1 | Project commands/results sent to the home authority | `docs/protocol/remote-control-v1.md` | `hq-protocol::remote_control::v1` |
| Nostr envelope v1 | Recipient binding and encrypted transport of canonical bytes | `docs/protocol/nostr-envelope-v1.md` | `hq-relay::envelope::v1` |
| Local API v1 | Lifecycle, domain RPC, errors, mutation retry, subscriptions | `docs/protocol/local-api-v1.md` | `hq-local-api::protocol::v1` |
| Harness contract v1 | Provider-neutral capabilities, lifecycle, delivery outcomes, events | `docs/harness-contract-v1.md` | `hq-harness` |
| Codex adapter baseline | Supported provider methods and decoding policy | versioned adapter spec/fixtures | `hq-codex` |
| Conformance trace v1 | Test-only deterministic operations and normalized observations | `docs/testing/conformance-v1.md` | `hq-testkit` |

Protocol DTOs live only in their owner modules. Domain types do not become wire protocols by
deriving serialization. All boundaries have explicit conversion and error classification.

The canonical and envelope specifications require exact test vectors. Local API and provider
protocols require semantic interoperability and bounded framing; their encodings need not resemble
Go. Test-only conformance traces never constrain production wire formats.

## Principal data flows

### Local fact-backed mutation

```text
CLI/TUI -> local API -> application command -> store actor transaction
  -> decide against snapshot -> event plan -> sign -> append -> reduce
  -> projections + outbox + receipt + revision -> commit
  -> invalidation + relay wake
```

If the response is lost, the client repeats the exact request and mutation ID. The receipt returns
the original result. Same ID with changed input fails.

### Inbound replicated fact

```text
relay frame -> envelope limits/verify/decrypt -> canonical limits/verify
  -> identity binding -> common store ingest transaction
  -> append/deduplicate -> reduce affected closure -> project/outbox/revision
  -> commit -> invalidation
```

Permanently malformed, unverifiable, wrong-recipient, or otherwise transport-inadmissible input is
bounded and quarantined for diagnosis. A verified semantic fact receives a reducer decision;
`unauthorized` and `unresolved` decisions never project. The canonical v1 specification will state
which non-projecting decisions remain in the fact set and their retention limits. Transient local
failures are staged for retry. No failure path modifies projections before successful reduction.

### Harness submission and output

```text
pending mailbox message -> durable claim -> stable provider submission ID
  -> checkpoint uncertain -> provider call
  -> accepted | rejected | reconcile-before-retry

provider event -> adapter normalization -> bounded neutral buffer
  -> deterministic event plan -> common store mutation
  -> durable message/activity -> delivery ledger checkpoint
```

Provider acceptance and local durable persistence are different boundaries. The ledger makes their
crash window explicit.

### Project workflow crossing external effects

```text
stable command -> durable preparing state -> perform/reconcile external effect
  -> commit authoritative success
  OR compensate to a documented stable state
  OR retain explicit unknown/blocked state for human resolution
```

No workflow reports success solely because its intent was stored. No retry repeats an external
effect without first reconciling the stable operation ID.

## Runtime ownership and lifecycle

Tokio owns asynchronous sockets, process pipes, timers, and coordination. SQLite remains on its
dedicated synchronous thread. Each long-lived component has one owner, a bounded mailbox, an
explicit lifecycle enum, and a documented shutdown acknowledgement.

The node cancellation tree is hierarchical:

```text
node
  local API listener
    client sessions
  store actor
  relay manager
    relay sessions
  harness supervisor
    logical workers
      provider process/transport
      persistence drain
  project workflow manager
```

Normal shutdown proceeds in order:

1. stop accepting new local clients, mutations, launches, and external workflow starts;
2. signal clients and workers that the node is draining;
3. stop relay intake after preserving accepted inbound work and durable outbound state;
4. stop provider request intake and drain accepted normalized output/activity;
5. reconcile or checkpoint in-flight sagas;
6. close store intake after all producers have completed, then commit/rollback and close SQLite;
7. wait for tracked tasks and escalate provider processes that miss their deadline; and
8. release the node lock and terminal resources.

Task abortion is last-resort escalation, not routine shutdown. Every operation documents whether
cancellation before, during, or after its awaited I/O leaves it rejected, committed, or uncertain.

## Verification design

Verification is specification-led and test-first. Every implementation phase begins by adding its
failing examples, laws, or state-machine model.

### Specification fixtures

Create reviewed fixtures for every canonical fact type, authority decision, causal status, project
transition, harness outcome, error category, and boundary size. Include missing and late parents,
duplicates, concurrent facts, revocation/regrant races, equal-time ordering, stale project heads,
resource conflicts, relay reconnects, provider partial frames, lost responses, and shutdown races.

Historical Go cases may be copied as scenario descriptions only after their expected outcome is
reviewed. The Go bytes, table rows, RPC responses, and UI snapshots are not golden Rust fixtures.

### Property and model tests

Generated causal DAGs and state-machine command sequences must establish:

- all nine algebraic laws;
- batch/incremental/repair equality;
- no projection support from unusable facts;
- complete and minimal maximal frontiers;
- revoke/regrant/reaccept authorization across every topological arrival order;
- parent-before-child and deterministic concurrent presentation order;
- mutation and provider-submission idempotency/collision behavior;
- project expected-head serialization and compensation invariants;
- bounded queue behavior under adversarial producers; and
- pagination concatenation equality with canonical conversation order without full-history work per
  page.

### Crash and lifecycle tests

Failpoints bracket canonical append, index update, projection update, outbox creation, receipt,
revision, wrapper persistence, relay acceptance, delivery-ledger checkpoints, provider output,
activity, saga transitions, and transaction commit. Every restart must recover to an old valid
state, a new valid state, or an explicit reconcilable uncertainty—never an unexplained hybrid.

Scripted relay and provider implementations make disconnects, partial frames, duplicates,
backpressure, and response loss deterministic. Real relays and installed providers are smoke tests,
not correctness foundations.

### Architecture and security tests

Automated checks enforce crate dependency direction, a single store-opening composition root, no
provider vocabulary in neutral crates, no SQL access from clients, no behavioral parsing of human
text/diagnostic fields, and no secret/environment serialization or logging.

### Performance gates

Record budgets for complete rebuild, high-fanout late-parent ingestion, long-conversation paging,
node readiness, invalidation-to-redraw, queue saturation, idle/active memory, and graceful shutdown.
The Rust implementation need not beat Go on every metric, but it must not hide algorithmic
regressions behind stronger types.

## Implementation sequence

The sequence below is dependency-driven. A phase is complete only when its exit gate is met; a
demo alone is not completion.

### 1. Freeze and specify the product

- Tag or otherwise record the final Go revision and stop feature development there.
- Build the retain/redesign/drop behavior ledger from the reference documents and remaining
  `PLAN.md` findings.
- Turn each retained algebra law and safety requirement into named acceptance cases.
- Write the canonical fact catalog at the semantic level, including every domain conflict rule.
- Resolve first-release scope and the explicit deferred list.

Exit gate: there are no uncategorized Go-facing compatibility assumptions, every fact family has a
semantic owner, and known Go defects are represented as Rust regression requirements rather than
expected old behavior.

### 2. Establish the Rust workspace and guardrails

- Create the workspace, common lint policy, formatting, dependency policy, CI, and architecture
  checks.
- Add `hq-domain`, `hq-reducer`, and `hq-testkit` with deterministic key, ID, clock, and DAG builders.
- Model IDs, bounded values, addresses, scopes, facts, commands, outcomes, and errors with no I/O.
- Add a minimal in-memory walking skeleton to prove the domain/protocol/application boundaries.

Exit gate: the core builds without Tokio/SQLite/Nostr/Ratatui dependencies, forbidden dependency
tests run in CI, and one small fact can be constructed and reduced entirely in memory.

### 3. Define and implement the causal kernel

- Specify reachability, usability, unresolved facts, frontiers, explicit authorities, projection
  support, and the canonical presentation comparator.
- Implement the complete batch reducer first.
- Implement domain reducers in dependency order: installation/local control facts; peer and mailbox
  capabilities; human accounts; messages and activity; agents/sessions; projects.
- Add generated DAG and domain state-machine tests before incremental optimization.

Exit gate: the batch reducer covers the complete first-release fact catalog and passes the nine
laws, authorization race matrix, project transition model, and canonical-order tests.

### 4. Specify and implement canonical protocols

- Write and internally review canonical fact v1 and remote-control v1 against this architecture.
- Implement strict decoding, deterministic encoding, signing, verification, bounds, and trust-type
  conversion.
- Add independent crypto vectors plus malformed, duplicate-field, non-canonical, tampered, and
  boundary fixtures.
- Ensure old Go schemas are rejected as unknown input rather than routed through compatibility code.

Exit gate: exact v1 vectors are stable, every semantic fact round-trips through explicit DTO
conversion, and fuzz/adversarial input cannot bypass trust transitions.

### 5. Build persistence, repair, and the atomic mutation path

- Design the new SQLite schema by data class, not by copying tables.
- Implement the dedicated store actor and complete batch rebuild first.
- Add receipts, revisions, canonical append, projections, dependency indexes, outbox intent,
  staging, quarantine, and operational ledgers.
- Implement affected-closure incremental reduction only after rebuild is correct.
- Implement the canonical ordered pagination index after the comparator is fixed.
- Add transaction failpoints and reopen/repair tests from the start.

Exit gate: every fact-backed mutation is atomic, repair is idempotent, incremental state equals a
fresh rebuild, paging equals reducer order, and all crash points recover lawfully.

### 6. Build application services, node lifecycle, and local API

- Implement transport-independent use cases and ports.
- Specify local API v1, typed errors, mutation replay, handshake, lifecycle, and subscriptions.
- Compose the store actor, local listener, ownership lock, cancellation tree, and task tracker in
  the node.
- Implement auto-start/readiness and reconnect/resubscribe behavior.
- Add a thin CLI for identity, basic queries, and local mutations against fake external adapters.

Exit gate: two local clients can race, lose responses, reconnect, resubscribe without a revision
gap, and shut/restart the node without duplicate mutations or leaked ownership.

### 7. Add encrypted replication

- Write and internally review Nostr envelope v1 against the canonical and transport invariants.
- Implement exact durable wrapper preparation, publish retry, retained catch-up, live subscription,
  auth, deduplication, staging, and quarantine.
- Keep healthy live sessions alive across ordinary publish wakes.
- Test two nodes against a deterministic relay, then a controlled real relay.

Exit gate: distinct Rust installations converge under reordering, duplication, disconnect,
offline catch-up, revoke/regrant traffic, and restart; no relay observation influences reduction.

### 8. Add the neutral harness and Codex adapter

- Approve harness contract v1 and its capability/reconciliation rules.
- Implement supervisor ownership, stable delivery, bounded normalization, interactive requests,
  persistence drain, and shutdown using a fake provider first.
- Implement the Codex adapter privately against a newly selected and pinned provider baseline.
- Convert normalized output/activity into the common fact mutation path.

Exit gate: the neutral conformance suite passes for the fake and Codex adapters; response-loss,
partial-frame, buffer-saturation, cancellation, process-exit, and drain tests have deterministic
evidence.

### 9. Complete project resources and external-effect sagas

- Add path canonicalization, overlap detection, health and cleanliness assessment, and advisory
  claims.
- Implement project activation, close, takeover, reassignment, dispatch, and worktree provisioning
  as explicit durable sagas.
- Test reconciliation and compensation at every filesystem/Git/provider boundary.

Exit gate: stale heads, conflicts, crashes, failed activation, forced transitions, and late runtime
output all produce one documented stable or reconcilable state with complete attribution.

### 10. Build the full CLI and Ratatui client

- Complete the CLI over the local client library; do not introduce direct shortcuts.
- Implement the pure TUI update/effect architecture and renderer.
- Add drafts, focus, logical scroll anchors, activity presentation, project flows, agent/session
  management, reconnect state, and actionable diagnostics.
- Add model tests, render snapshots at representative sizes, and terminal restoration tests.

Exit gate: all retained workflows are available through supported clients, survive reconnect and
invalidation, and do not bypass application services.

### 11. System qualification and cutover readiness

- Run the full fixture, property, fuzz, model, crash, lifecycle, security, and performance suites.
- Dogfood with new identities and new state directories on controlled relays.
- Exercise backup, restart, offline catch-up, relay loss, provider crash, database repair, and node
  replacement drills.
- Write and rehearse a procedure that archives the Go binary, key, database, and logs read-only
  without allowing Rust to open or mutate them.
- Produce the evidence an operator needs to begin a soak period and make Rust the sole active
  product; do not perform that externally consequential cutover without separate authorization.

Exit gate: every row of the acceptance matrix has evidence and no required workflow depends on the
Go process, data, protocol, or implementation.

## Acceptance matrix

| Area | Required evidence |
| --- | --- |
| Domain/algebra | Nine laws, reviewed conflict rules, generated DAGs, maximal-frontier cases |
| Canonical protocol | Exact v1 vectors, strict/adversarial corpus, signature and size boundaries |
| Authorization | Capability and account grant/revoke/regrant/observe race matrix |
| Persistence | Atomic mutation failpoints, repair equality, incremental/batch equality |
| Queries | Canonical ordering, stable cursors, sublinear later-page behavior |
| Local API/node | Replay, reconnect, revision-race closure, readiness, graceful restart |
| Relay | Exact wrapper retry, duplicates, auth, disconnect, catch-up, session wake behavior |
| Harness | Capability validation, uncertain delivery, output ordering, backpressure, drain/kill |
| Projects | Transition model, expected-head races, resource conflicts, saga compensation |
| TUI/CLI | Retained workflow coverage, pure model tests, render snapshots, terminal restoration |
| Security/operations | Secret redaction, bounded storage/queues, performance budgets, recovery drill |

## Decisions that remain before coding

The architecture does not depend on the answers below, but the relevant implementation phase does:

- the exact Rust-era canonical JSON/event schema and provisional Nostr kind;
- the exact local API framing and whether the node is packaged as one or two executables;
- the initial operating-system support target for local IPC;
- the Rust-era Codex baseline and which provider capabilities remain first-release requirements;
- the final retained CLI/TUI workflow list;
- whether identity export/import is in the first Rust release; and
- quantitative performance and soak-period thresholds.

These are specification decisions, not reasons to preserve Go encodings. They should be recorded
as small ADRs when resolved.

## Definition of done

The rewrite is complete when the Rust system independently satisfies the reviewed product
requirements and algebraic contract, all required protocols have Rust-era specifications, all
durable and external-effect boundaries have recovery evidence, and normal operation has no code
path or data dependency on Go.

The measure of success is not fewer lines or superficial feature resemblance. It is that causal
authority, convergence, atomicity, lifecycle ownership, and domain state transitions are explicit,
testable, and difficult to represent incorrectly.
