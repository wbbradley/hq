# HQ Rust storage contract

Status: normative persistence specification

HQ storage v2 is a new Rust-owned SQLite database. It does not open, migrate, repair, reset, or
otherwise interpret a Go database. The database has application ID `0x48515253` (`HQRS`) and user
version `2`; any other nonempty SQLite file is incompatible normal-startup input. Because no Rust
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

| Class | Storage v1 ownership | Rebuildable |
| --- | --- | ---: |
| Canonical knowledge | Exact verified signed event bytes keyed by content-derived fact ID | No |
| Canonical evidence indexes | Normalized parent and typed historical-authority edges | No; verified against exact signed bytes on every corpus load |
| Deterministic reduction indexes | Reverse dependencies, decisions, diagnostics, conflicts, and reducer order | Yes, only through an explicit repair operation |
| Materialized projections | Reserved for reducer reports and query rows | Yes |
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
deletes only the rebuildable reduction tables, writes every normalized replacement group, reads the
typed index back through private fixed-width and closed-vocabulary row codecs, and requires exact
snapshot and digest equality before commit. Canonical facts, exact event bytes, canonical parent and
authority rows, schema metadata, and future durable operational tables are outside the repair
allowlist. Dropping or failing the transaction at any replacement or verification checkpoint leaves
the preceding complete index intact, and repeating a successful repair is idempotent.

`load_reduction_index()` is read-only and returns the last successful repair even when newer
canonical facts have since arrived; callers request an explicit repair when they need those facts
reconsidered. A database with no completed reduction index returns `NotRepaired`. Partial,
out-of-vocabulary, oversized, cross-domain, noncontiguous, or digest-inconsistent rebuildable rows
return `RebuildableStateCorrupt`. Neither case triggers implicit repair. This makes the authoritative
batch/rebuild boundary visible to later projection and incremental-reduction packages.

The correctness-first oracle currently clones the bounded semantic corpus for the four reducers.
That cost is deliberate for repair equality; measured shared-report or incremental optimization is
deferred to the scaling package.
