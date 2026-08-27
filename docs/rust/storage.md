# HQ Rust storage contract

Status: normative persistence specification

HQ storage v9 is a new Rust-owned SQLite database. It does not open, migrate, repair, reset, or
otherwise interpret a Go database. The database has application ID `0x48515253` (`HQRS`) and user
version `9`; any other nonempty SQLite file is incompatible normal-startup input. Because no Rust
release has shipped yet, schema evolution advances the fresh-database identity rather than adding
an in-place migration path.

## Ownership and durability

One dedicated synchronous `hq-store` thread owns the only `rusqlite` connection. Callers exchange
coarse typed commands and one-shot results through a bounded mailbox. Connections, transactions,
statements, SQL, and row codecs remain private to the adapter. The node retains the state-directory
lock for the store lifetime and shuts store intake before joining this thread.

The database uses a private `0700` state directory and `0600` database file, WAL journaling, full
synchronous durability, foreign-key enforcement, disabled trusted schemas, defensive mode, an
integrity check on open, and checkpoint-on-close. Secure local permissions reduce accidental and
other-user disclosure; they do not encrypt domain content or claim a security boundary against
another process running as the same operating-system user.

## Data classes

| Class | Storage v9 ownership | Rebuildable |
| --- | --- | ---: |
| Canonical knowledge | Exact verified signed event bytes keyed by content-derived fact ID | No |
| Canonical evidence indexes | Normalized parent and typed historical-authority edges | No; verified against exact signed bytes on every corpus load |
| Deterministic reduction indexes | Reverse and affected dependencies, decisions, diagnostics, conflicts, global reducer order, and conversation-local order | Yes, through atomic ingest or explicit repair |
| Materialized projections | Complete authority, conversation/activity, named-agent, and project frontiers, typed values, ordered children, and support | Yes, through atomic ingest or explicit repair |
| Durable operational state | Mutation receipts, canonical commit revisions, change revision, and canonical outbox intents; delivery, cursors, and saga checkpoints remain reserved | No |
| Ephemeral runtime state | Sockets, tasks, environments, UI caches | Never stored as domain state |
| Rejected/temporary input | Reserved bounded quarantine or retry staging with no domain effect | No domain effect |

The corpus schema contains one fixed schema marker plus `canonical_facts`, `fact_parents`, and
`fact_authorities`. Missing parent facts are legal: an edge records signed causal dependency, while
the reducer decides whether the dependent fact is usable.

## Immutable corpus rules

Ingest accepts only `VerifiedSemanticFact`, after raw bounds, strict outer parsing, event identity,
BIP-340 signature, supported-prefix dispatch, canonical DTO, and intrinsic semantic validation have
all succeeded. A new fact transaction inserts its exact signed event and normalized causal indexes.
An equal replay returns its original commit revision without another write. Reusing one fact ID with different event bytes, namespace, family,
parents, or authorities is an immutable identity collision and fails closed.

Load orders by fact ID and does not deserialize domain structs from database rows. It reruns the
entire protocol trust pipeline from exact event bytes, then compares the reconstructed fact ID,
namespace, family, parents, and authority edges with stored values. Invalid signatures, unsupported
content, malformed evidence, partial indexes, and mismatches fail the load. A later explicit repair
operation may replace rebuildable rows after deriving them from this reverified corpus; normal load
never silently edits evidence or indexes.

## Atomic canonical ingest

`ingest_verified(fact, policy)` is the only production entry for verified canonical facts. One
immediate SQLite transaction checks durable canonical-commit lineage, appends exact evidence and
causal indexes, reverifies the transaction-visible corpus, runs the complete four-domain reducer
oracle, incrementally patches and reads back every rebuildable package, allocates a full-width change revision,
derives authorized per-recipient outbox intents, stores fact-to-revision lineage, and commits. An
error at any pre-commit boundary drops the transaction and leaves the preceding complete state.

Outbox derivation requires an admitted decision. Installation-private facts never leave the local
installation. Peer-addressed facts target the peer installation. Account and control facts derive
the creator and every active membership from the post-reduction authority snapshot; control facts
also include the target home. The verified author and policy-local installation are removed and the
resulting installation set is deduplicated.

`canonical_commits` binds every ingested fact to its original revision and is outside repair. An
exact duplicate verifies immutable equality and returns that revision before reduction, projection,
outbox, revision, or invalidation work. After a successful new commit, the worker updates an atomic
latest-revision value and attempts a capacity-one wake without blocking. Multiple pending changes
coalesce; a disconnected or backpressured observer cannot fail or delay the durable commit. A lost
response is recovered by exact replay through the same canonical lineage.

## Transaction-consistent local mutations

`execute_local_mutation(request)` is the fact-backed local mutation boundary. Its bounded request
owns a stable command ID and request digest, explicit authority policy, a shared signer capability
that exposes no secret bytes, and a one-shot decision callback. One immediate transaction first
loads the receipt. Equal input returns its exact retained kind, bytes, and original revision before
the callback; a changed digest under the same command ID is `MutationConflict`.

For a new command, storage reverifies and completely reduces the transaction-visible corpus, then
invokes the callback with that snapshot. A committed decision supplies a typed
`CanonicalEventPlan`, explicit BIP-340 auxiliary randomness, and bounded exact result bytes. Storage
canonically authors and verifies the event, calls the same transaction-owned append/reduce/project/
outbox/lineage engine as remote ingest, requires the new fact to be admitted, and stores the
committed receipt at that engine's revision. The fact, dependency rows, every projection package,
outbox intents, lineage, receipt, and revision therefore commit together or all roll back.

A rejected decision writes no canonical fact. It atomically allocates a revision and stores the
exact rejected receipt so response loss remains reconcilable. New committed and rejected local
transactions publish the same capacity-one post-commit invalidation; retry and conflict publish
nothing. Repair cannot alter receipts or revisions. Unsigned local configuration, repair, and later
operational saga mutations remain separately named operations and cannot enter an optional-fact
variant of this path.

## Complete-batch oracle

`complete_snapshot(policy)` performs one actor-owned corpus read and full protocol reverification,
then passes reducer-ready semantic values across the pure boundary. It runs the authority,
conversation/activity, named-agent, and project reducers over that same corpus with the same
caller-supplied `AuthorityPolicy`. Its typed `CompleteSnapshot` contains all four authoritative
reports; it contains neither SQL rows nor serialized reducer structs and does not mutate storage.

The reports normalize into one `ReductionIndexSnapshot`. The snapshot has a closed reduction-domain
enum and retains the global reverse-dependency graph (including missing vertices), a conservative
affected graph, every per-domain
fact decision, missing and unusable dependencies, failed historical-authority roles, conflict
participants, deterministic dependency order, reducer-owned presentation order, and exact
conversation-local orders. Framework and
domain reasons use explicit exhaustive integer codecs, including nested authority reasons and typed
role parameters. Debug prose and generic domain serialization are not persistence formats.

## Incremental materialization

Every complete report exposes aggregate membership for projected and unusable facts. Storage
normalizes undirected causal relationships, shared aggregate membership, transitive projection
support, and conflict participants into `reduction_affected_dependencies`. Selection starts with
the new fact and traverses the union of the persisted and fresh graph, so a disappearing conflict
or support edge cannot hide a required retraction. A policy change conservatively selects every
known vertex. Every changed decision, frontier, projection, and support set must cite an affected
identity or the transaction fails with `ReductionFailed`.

`reduce_complete` remains the one executable policy definition and the fresh result is the
continuous equality oracle. Storage stages that expected representation in an isolated in-memory
schema through the same strict relational codecs, compares typed SQLite values by primary key, and
applies only exact differences to the live transaction. Removed and same-key changed rows are
deleted child-first, while changes and additions are inserted parent-first with deferred
foreign-key checking. No unchanged row is deleted or rewritten. The complete structural and four
domain packages are then loaded and required to equal the batch snapshots before operational state
can advance. This avoids a second, drifting partial implementation of domain policy while making
ordinary materialization writes incremental.

## Indexed conversation pages

`ConversationKey` is a closed thread or provider-session identity with an exact installation-
qualified counterparty mailbox. For every key, repair and ingest select only projected messages and
selected/durable activity values, then invoke the reducer's canonical Kahn comparator on that
induced conversation graph. `reduction_conversation_order` stores conversation-local positions and
stable fact IDs; there is no dense global display rank and clients never recreate a lookalike sort.

`load_conversation_entries(key, limit, cursor)` returns a typed `Page<ConversationEntry>` whose
closed union contains either a complete `MessageView` or `ActivityView`. Limits are `1..=200`.
The strict `v1` cursor binds the SHA-256 conversation-key digest to the last returned fact ID; a
malformed, stale, or cross-conversation anchor is rejected. A later page resolves that fact's
current local position through the unique covering index, selects at most `limit + 1` order rows,
and hydrates at most `limit` exact projections. It never loads the canonical corpus, loads a full
projection snapshot, or sorts conversation history.

## Explicit repair

`repair(policy)` first computes the complete oracle without writes. It then opens one transaction,
deletes only the rebuildable reduction, authority, conversation/activity, named-agent, and project
tables, writes every normalized replacement group, reads all five typed snapshots back through private fixed-width and
closed-vocabulary row codecs, and requires exact snapshot and digest equality before commit.
Canonical facts, exact event bytes,
canonical parent and authority rows, schema metadata, and durable operational tables are
outside the repair allowlist. Dropping or failing the transaction at any replacement or verification
checkpoint leaves the preceding complete structural/authority/conversation/agent/project set intact, and
repeating a successful repair is idempotent.

`load_reduction_index()` is read-only and returns the last successful ingest or explicit repair. A
database with no completed reduction index returns `NotRepaired`. Partial,
out-of-vocabulary, oversized, cross-domain, noncontiguous, or digest-inconsistent rebuildable rows
return `RebuildableStateCorrupt`. Neither case triggers implicit repair. This keeps the
authoritative batch/rebuild boundary explicit while ordinary ingest uses the continuously checked
incremental patch path.

## Durable operational primitives

`mutation_receipts` binds one 32-byte command identity to its 32-byte exact-request digest, a
closed committed/rejected result kind, bounded exact result bytes, and the transaction's revision.
An equal insertion is idempotent; any unequal reuse is a mutation conflict. These bytes use the
strict application-owned v1 outcome encoding documented in `docs/rust/application-services.md`,
not serialized Rust domain structs or diagnostic prose. The application gateway requires the
decoded committed/rejected outcome to agree with the separately stored closed result kind.

`change_revision` stores the full unsigned 64-bit revision as fixed-width big-endian bytes, so it
does not silently lose the upper half of the domain in SQLite's signed integer representation.
Allocation is monotonic and fails explicitly at `u64::MAX` rather than wrapping. Public reads expose
the typed `Revision` only.

`outbox_intents` has one identity per canonical fact and recipient installation. It retains the
exact bounded signed canonical event bytes and creating revision so later encryption and relay
retries never reconstruct canonical evidence. Unequal reuse of that identity fails closed. Public
queries are deterministically ordered and capped at 1,024 rows; transport wrappers and delivery
attempts will extend operational state without changing canonical identity. Explicit repair never
deletes, rewrites, or derives any receipt, revision, or outbox row.

## Authority projections

`AuthorityProjectionSnapshot` is the application-owned representation-independent query boundary
for the complete authority report. It owns ordered typed maps for every `AuthorityAggregateKey` frontier,
every `AuthorityProjectionKey` and value, and every transitive support set. Callers can inspect
installations, installation-qualified mailboxes, directional peer-route histories, mailbox
capabilities, account roots, device memberships, and the policy-local account-selection register
without SQL access or another reducer run. `load_authority_snapshot()` is read-only and verifies
that the structural half of the same atomic ingest or repair is intact.

`Store::authoritative_snapshot()` is one actor request that reads the current revision and all four
application projection packages at one serialized store point. `StoreGateway` implements the
application query and fact-commit ports with explicit authority policy and signer capabilities;
other application capabilities remain outside persistence ownership.

Authority values are not serialized Rust structs. Dedicated strict tables and normalized child rows
store each projection variant: route candidates, blocks, relay locators and frontiers; capability
revoke and observed-action facts; membership grants, relay locators, acceptances, revokes and
frontiers; selection candidates; aggregate frontiers; and projection support. Private exhaustive
codecs map every key, state, address, public key, bounded label/error code, relay scheme/value, and
child relation. Loading reconstructs values through typed constructors, enforces fixed widths,
bounds, ordinals, key/value pairing, parent/child ownership and row-count limits, and verifies a
digest over every authority row before returning it. Unknown, partial, orphaned, duplicated,
cross-key, oversized, or valid-looking changed rows return `RebuildableStateCorrupt`; repair remains
the only recovery path.

## Conversation and activity projections

`ConversationProjectionSnapshot` is the SQL-independent persisted query boundary for the complete
conversation/activity report. It owns typed ordered maps for all `ConversationAggregateKey`
frontiers, all six `ConversationProjectionKey` variants and values, and every transitive support
set. `load_conversation_snapshot()` first validates the structural and authority packages from the
same commit, then returns the last atomically ingested or explicitly repaired conversation view
without rerunning a reducer.

Storage uses explicit master rows for all aggregate and projection key variants. Composite activity
and retention namespaces retain source installation/mailbox, provider, session, operation, optional
item, activity kind, logical key, and runtime in individually validated columns; a private digest is
only a relational identity and is recomputed from those columns on load. Dedicated parent and child
tables retain thread roots, answers, cancellations, pairwise causal relations and ready order;
typed message content, optional recipient/correlation/project shapes, reversible state frontier and
peer receipt evidence; action-group order and final answer; selected snapshots and permanent
completed records; the progress-retention order and total; aggregate frontiers; and support.

Private exhaustive codecs cover every closed message purpose, presentation kind, activity kind and
status, causal relation, boolean, fixed identity, bounded content/provider/session/error value, and
optional shape. Positive activity sequence numbers use fixed-width big-endian bytes so the complete
`NonZeroU64` domain round-trips despite SQLite's signed integer range. Ordered children require
unique contiguous zero-based positions; thread relation and ready-answer sets, final-answer
membership, rejection/open state, completed-record identity, and the 200-item retention budget are
checked on reconstruction. Counts are bounded before allocation, and a digest covers every explicit
conversation row. Unknown, partial, orphaned, cross-key, noncontiguous, oversized, or
constraint-valid changed rows return `RebuildableStateCorrupt` until explicit repair.

## Named-agent projections

`AgentProjectionSnapshot` is the SQL-independent persisted query boundary for the complete
named-agent report. It owns ordered typed maps for every `AgentAggregateKey` frontier, all seven
`AgentProjectionKey` variants and values, and every transitive support set.
`load_agent_snapshot()` first validates the structural, authority, and conversation packages from
the same commit, then returns the last atomically ingested or explicitly repaired named-agent view.

Dedicated tables retain permanent name claims; normalized agent lifecycle, mailbox and retirement
sets; immutable provider-session binding histories; grow-only repository-context histories and
frontiers; durable selection candidates and active values; rename candidates, resolution and clear
state; direct-session bindings; aggregate frontiers; and projection support. Composite keys keep
agent, installation-qualified mailbox, provider, and provider-session fields in explicit validated
columns; private digests are recomputed relational identities rather than serialized domain state.

Private exhaustive codecs reconstruct nested repository contexts and optional resource locators,
bounded names/provider/session/branch values, closed locator schemes and lifecycle states, booleans,
and fixed-width identities. Loading checks projection key/value pairing, name and session conflict
semantics, lifecycle and runnable consistency, selected-candidate membership, rename resolution and
clear semantics, frontier/history membership, contiguous ownership, counts, and a digest over every
agent row. Unknown, partial, orphaned, oversized, cross-key, or constraint-valid changed rows return
`RebuildableStateCorrupt`; explicit repair is the only recovery path.

## Project projections

`ProjectProjectionSnapshot` completes the SQL-independent persisted query boundary for all four
reducers. It owns ordered maps for every `ProjectAggregateKey` frontier, all five
`ProjectProjectionKey` variants and values, and every transitive support set.
`load_project_snapshot()` validates the structural, authority, conversation, and agent packages
from the same transaction before returning the last atomically ingested or repaired project view.
One successful repair proves every
persisted projection report exactly equals the fresh complete-batch oracle.

Explicit tables retain project roots, heads and fork participants; desired resources, typed health,
primary choice, active claims and cross-project conflicts; assignment bindings, configuring/
runnable/blocked phases and support; accepted inputs and full-width sequences; immutable dispatch
attribution; output binding, typed message content and collision status; remote-command queued,
received, terminal and conflicted stages; aggregate frontiers; and projection support. Composite
resource and assignment namespaces keep their components in validated columns behind recomputed
private digests.

Private exhaustive codecs validate fixed identities, bounded text, resource schemes and health,
project lifecycle, assignment phase, message purpose/presentation, output status, command result,
runtime observation, every optional shape, full-width `u64` sequences, and key/value pairing.
Loading enforces mailbox/home identity, primary and claim membership, claimability and lifecycle
rules, assignment-runnable consistency, nested provenance shapes, row bounds, ownership, counts,
and a digest over every explicit project row. Unknown, partial, orphaned, oversized, cross-key, or
constraint-valid changed rows return `RebuildableStateCorrupt` until explicit repair.

The correctness-first oracle still clones the bounded semantic corpus for the four reducers. That
deliberate compute cost buys continuous equality; incremental materialization avoids clearing or
rewriting unrelated relational state, and indexed conversation reads meet the independent query
scaling gate.
