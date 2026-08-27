# HQ Rust storage contract

Status: normative persistence specification

HQ storage v1 is a new Rust-owned SQLite database. It does not open, migrate, repair, reset, or
otherwise interpret a Go database. The database has application ID `0x48515253` (`HQRS`) and user
version `1`; any other nonempty SQLite file is incompatible normal-startup input.

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
| Deterministic indexes | Normalized parent and typed historical-authority edges | Yes, only through an explicit repair operation |
| Materialized projections | Reserved for reducer reports and query rows | Yes |
| Durable operational state | Reserved for receipts, revisions, outbox, delivery, cursors, and saga checkpoints | Generally no |
| Ephemeral runtime state | Sockets, tasks, environments, UI caches | Never stored as domain state |
| Rejected/temporary input | Reserved bounded quarantine or retry staging with no domain effect | No domain effect |

The initial corpus schema contains one fixed schema marker plus `canonical_facts`, `fact_parents`,
and `fact_authorities`. Missing parent facts are legal: an edge records signed causal dependency,
while the reducer decides whether the dependent fact is usable.

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
