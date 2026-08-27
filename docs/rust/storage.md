# HQ Rust storage contract

Status: normative persistence specification

HQ storage v6 is a new Rust-owned SQLite database. It does not open, migrate, repair, reset, or
otherwise interpret a Go database. The database has application ID `0x48515253` (`HQRS`) and user
version `6`; any other nonempty SQLite file is incompatible normal-startup input. Because no Rust
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

| Class | Storage v6 ownership | Rebuildable |
| --- | --- | ---: |
| Canonical knowledge | Exact verified signed event bytes keyed by content-derived fact ID | No |
| Canonical evidence indexes | Normalized parent and typed historical-authority edges | No; verified against exact signed bytes on every corpus load |
| Deterministic reduction indexes | Reverse dependencies, decisions, diagnostics, conflicts, and reducer order | Yes, only through an explicit repair operation |
| Materialized projections | Complete authority, conversation/activity, named-agent, and project frontiers, typed values, ordered children, and support | Yes, only through explicit repair |
| Durable operational state | Reserved for receipts, revisions, outbox, delivery, cursors, and saga checkpoints | Generally no |
| Ephemeral runtime state | Sockets, tasks, environments, UI caches | Never stored as domain state |
| Rejected/temporary input | Reserved bounded quarantine or retry staging with no domain effect | No domain effect |

The corpus schema contains one fixed schema marker plus `canonical_facts`, `fact_parents`, and
`fact_authorities`. Missing parent facts are legal: an edge records signed causal dependency, while
the reducer decides whether the dependent fact is usable.

## Immutable corpus rules

Append accepts only `VerifiedSemanticFact`, after raw bounds, strict outer parsing, event identity,
BIP-340 signature, supported-prefix dispatch, canonical DTO, and intrinsic semantic validation have
all succeeded. A new fact transaction inserts its exact signed event and normalized causal indexes.
An equal replay is idempotent. Reusing one fact ID with different event bytes, namespace, family,
parents, or authorities is an immutable identity collision and fails closed.

Load orders by fact ID and does not deserialize domain structs from database rows. It reruns the
entire protocol trust pipeline from exact event bytes, then compares the reconstructed fact ID,
namespace, family, parents, and authority edges with stored values. Invalid signatures, unsupported
content, malformed evidence, partial indexes, and mismatches fail the load. A later explicit repair
operation may replace rebuildable rows after deriving them from this reverified corpus; normal load
never silently edits evidence or indexes.

## Complete-batch oracle

`complete_snapshot(policy)` performs one actor-owned corpus read and full protocol reverification,
then passes reducer-ready semantic values across the pure boundary. It runs the authority,
conversation/activity, named-agent, and project reducers over that same corpus with the same
caller-supplied `AuthorityPolicy`. Its typed `CompleteSnapshot` contains all four authoritative
reports; it contains neither SQL rows nor serialized reducer structs and does not mutate storage.

The reports normalize into one `ReductionIndexSnapshot`. The snapshot has a closed reduction-domain
enum and retains the global reverse-dependency graph (including missing vertices), every per-domain
fact decision, missing and unusable dependencies, failed historical-authority roles, conflict
participants, deterministic dependency order, and reducer-owned presentation order. Framework and
domain reasons use explicit exhaustive integer codecs, including nested authority reasons and typed
role parameters. Debug prose and generic domain serialization are not persistence formats.

## Explicit repair

`repair(policy)` first computes the complete oracle without writes. It then opens one transaction,
deletes only the rebuildable reduction, authority, conversation/activity, named-agent, and project
tables, writes every normalized replacement group, reads all five typed snapshots back through private fixed-width and
closed-vocabulary row codecs, and requires exact snapshot and digest equality before commit.
Canonical facts, exact event bytes,
canonical parent and authority rows, schema metadata, and future durable operational tables are
outside the repair allowlist. Dropping or failing the transaction at any replacement or verification
checkpoint leaves the preceding complete structural/authority/conversation/agent/project set intact, and
repeating a successful repair is idempotent.

`load_reduction_index()` is read-only and returns the last successful repair even when newer
canonical facts have since arrived; callers request an explicit repair when they need those facts
reconsidered. A database with no completed reduction index returns `NotRepaired`. Partial,
out-of-vocabulary, oversized, cross-domain, noncontiguous, or digest-inconsistent rebuildable rows
return `RebuildableStateCorrupt`. Neither case triggers implicit repair. This makes the authoritative
batch/rebuild boundary visible to later projection and incremental-reduction packages.

## Authority projections

`AuthorityProjectionSnapshot` is the representation-independent persisted query boundary for the
complete authority report. It owns ordered typed maps for every `AuthorityAggregateKey` frontier,
every `AuthorityProjectionKey` and value, and every transitive support set. Callers can inspect
installations, installation-qualified mailboxes, directional peer-route histories, mailbox
capabilities, account roots, device memberships, and the policy-local account-selection register
without SQL access or another reducer run. `load_authority_snapshot()` is read-only, verifies that
the structural half of the same repair is intact, and retains the explicit stale-until-repair
semantics of `load_reduction_index()`.

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
same repair, then returns the last explicitly repaired conversation view without rerunning a
reducer. A later append therefore leaves the prior snapshot readable and intentionally stale until
the caller repairs.

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
the same repair, then returns the last explicitly repaired named-agent view. A later append remains
invisible to this query until the caller explicitly repairs.

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
from the same transaction before returning the last explicitly repaired project view. Appends leave
that view intentionally stale until explicit repair, and one successful repair proves every
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

The correctness-first oracle currently clones the bounded semantic corpus for the four reducers.
That cost is deliberate for repair equality; measured shared-report or incremental optimization is
deferred to the scaling package.
