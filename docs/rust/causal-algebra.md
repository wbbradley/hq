# HQ Rust causal fact algebra

Status: normative semantic specification

This document defines the pure algebra consumed by the Rust reducer. It is independent of JSON,
Nostr kinds, signatures, SQLite tables, local API methods, arrival timestamps, and runtime tasks.
The semantic types described here are mapped explicitly by later protocol specifications.

Related sources are the product boundary in `docs/rust/behavior-ledger.md`, the fact inventory in
`docs/rust/semantic-fact-catalog.md`, and executable cases in
`docs/rust/acceptance-scenarios.md`. The implemented authority specialization is documented in
`docs/rust/authority-model.md`.

## Semantic universe and notation

A `KnowledgeItem` is either a supported semantic record or a cryptographically verified opaque
record whose protocol/version/family is not supported. Supported records are tagged as
`canonical` or `remote-control`; their protocol spaces and identifiers cannot be confused.

Let:

- `E` be a finite map from typed knowledge ID to exact immutable knowledge item;
- `F(E)` be the supported canonical facts in `E`;
- `C(E)` be the supported remote-control records in `E`;
- `parents(f)` be the set of declared causal dependencies of `f`;
- `authorities(f)` be typed authority references, each of which must also be in `parents(f)`;
- `R(E, policy)` be the complete normalized reduction report for an explicit local policy; and
- `P(E, policy)` be the public semantic projections contained in that report.

`policy` contains only explicit semantic inputs such as the local installation identity and the
reserved local-human mailbox identity. It contains no clock, randomness, receipt order, database
row identity, network observation, filesystem state, or provider state.

A supported fact contains semantic values only:

```text
Fact {
  id: typed content-derived identity,
  protocol_class: canonical | remote-control,
  author: installation and signer identity,
  authored_at: signed presentation timestamp,
  scope: installation-private | peer-addressed | account-addressed,
  parents: set<KnowledgeId>,
  authorities: map<AuthorityRole, nonempty set<KnowledgeId>>,
  payload: one cataloged semantic variant
}
```

The protocol layer proves exact identity and signature before constructing this value. The reducer
still validates semantic signer, scope, address, parent-role, authority, aggregate, and transition
rules. An equal knowledge ID with unequal exact verified content is a collision and never becomes
semantic knowledge.

## Required causal dependencies

Every declared parent is a required dependency, not an optional ordering hint. Typed parent roles
identify why a dependency exists:

- aggregate root or previous head;
- thread root or message-state target;
- authority grant, membership, or creator root;
- observation or revoke frontier;
- current multivalue frontier being resolved;
- accepted project input or active assignment; or
- remote command/request/result correlation.

Every authority reference must occur in the parent set, but an ordinary parent is not thereby an
authority. Authority evaluation matches the fact family, authority role, subject, audience,
signer, and causal point exactly. Adding an unrelated, valid descendant to `parents` cannot grant a
permission or replace a missing typed authority.

The semantic catalog states the minimum parent roles for every variant. A writer may add useful
causal context, but all added parents become required and must be usable before the child can
support a projection.

## Structural reachability

The structural graph has one vertex for every supported item and an edge `p -> f` for every
declared `p` in `parents(f)`, including a parent not yet present in `E`. Structural reachability is:

```text
a <=s b  when a = b or a reaches b through declared parent edges
a <s  b  when a <=s b and a != b
a ||s b  when neither a <=s b nor b <=s a
```

Structural reachability answers graph questions and detects missing dependencies. It does not by
itself grant authority or semantic support. A self-parent, a detected cycle among present items, a
parent in the wrong typed protocol role, or an identifier collision is structurally invalid.

An edge to an absent parent is not a cycle and does not make the child invalid. It makes the child
unresolved until that parent is known.

## Usable reachability

A fact is `usable` only when it is intrinsically valid, every declared parent exists and is usable,
its typed authority checks pass against the complete fact set, and its aggregate conflict/transition
rules admit it. Usable reachability is structural reachability restricted to usable paths:

```text
a <=u b  when every vertex and edge on a structural path from a to b is usable
```

Dominance, authority history, frontiers, projection support, and project sequencing use `<=u`.
An invalid, unsupported, unauthorized, conflicted, or unresolved fact cannot carry causality into
a supported child. This prevents an unusable bridge from laundering authority.

Usability is a result of complete-set reduction, not a permanent flag assigned at arrival. Adding
a revoke, conflicting root, resolving parent, or conflict-resolution fact may change a prior
decision and retract or restore its projection.

## Fact decisions

Every knowledge item has exactly one normalized decision in `R(E, policy)`:

| Decision | Meaning | May support a projection? | May change after `E` grows? |
| --- | --- | ---: | ---: |
| `projected` | Supported, valid, fully resolved, authorized, and admitted by its domain rules. It may be historical rather than active. | yes | yes |
| `unresolved` | At least one required parent is absent or currently unusable. The report lists missing and present-unusable blockers separately. | no | yes |
| `unauthorized` | Dependencies are available, but the exact signer/audience/authority rule fails at the fact's causal point. | no | yes |
| `conflicted` | The fact participates in an explicit unique-root, multivalue, linear-fork, identity-collision, or global-cardinality conflict that forbids a single active result. | only as conflict/audit evidence | yes |
| `invalid` | The supported semantic payload, graph shape, address, signer relationship, or state transition is intrinsically impossible. | no | only if the reason was a presently unusable dependency mistakenly classified; correct reducers use `unresolved` for that case |
| `unsupported` | Cryptography and outer identity are verified, but the protocol version or semantic family is not implemented. | no | only after a future binary adds support |

Deduplication is not a decision. Repeating identical knowledge produces one item and a duplicate
ingest observation. Malformed bytes and failed cryptography never become `KnowledgeItem`; ingestion
reports them as rejected input outside `R`.

Reason codes are closed semantic enums. Human prose may accompany a diagnostic outside the core,
but no reducer branch parses or compares that prose.

## Dependency readiness and reconsideration

For each non-projected item the report records:

- absent required IDs;
- present but unusable required IDs and their current decision codes;
- failed typed authority roles;
- aggregate conflict identity and all participants; and
- the reverse dependants that must be reconsidered if this decision changes.

When knowledge grows, a correct incremental caller recomputes the reverse-dependent closure and
every affected aggregate/global constraint. It does not merely retry the newly arrived item.
Authority revokes, unique-root conflicts, resource overlaps, and agent cardinality can affect facts
that are not graph descendants, so their indexes contribute additional affected edges.

## Causal frontiers

For aggregate `A`, let `U_A` be all usable facts belonging to `A`. Its frontier is exactly:

```text
frontier(A) = { f in U_A : there is no g in U_A where f <u g }
```

The frontier retains every concurrent maximum. Sorting a frontier is allowed only for normalized
output; sorting never chooses a winner. A writer that resolves a multivalue state or supersedes a
remove-wins state must cite every relevant current maximum. Citing one lexicographically convenient
member leaves the other maxima concurrent and therefore unresolved or removed according to policy.

Frontiers are aggregate-specific. A descendant in an unrelated aggregate does not remove a fact
from the frontier being evaluated.

## Conflict registers and domain rules

Every catalog entry chooses one of these explicit patterns or a more specific rule:

| Pattern | Concurrent behavior | Resolution |
| --- | --- | --- |
| Grow-only set | Preserve every distinct usable member. | No winner is needed. |
| Unique root | Two unequal roots for one semantic ID conflict; the aggregate has no active root. | First release exposes the permanent conflict; no identifier/timestamp winner. |
| Multivalue register | Preserve every causal maximum. A consumer requiring one value is blocked. | A later fact cites every maximum and supplies one new value. |
| Remove-wins register | If any maximum is a remove, block/archive/inactive wins over concurrent add/use/restore. | A later add/restore must descend from every maximal remove. |
| Absorbing removal | Any usable retirement or rejection permanently removes the subject from active use. | No first-release resurrection fact exists. |
| Home-linear log | One unique root and exactly one child of each head are admitted. Sibling children form an explicit fork. | First release exposes the fork and applies neither branch beyond the common head. |
| Sequenced snapshot | Higher source sequence wins among concurrent maxima from the same source/runtime namespace; unequal values at one sequence conflict. | A later causally descending higher sequence resolves. |
| Global cardinality | Every conflicting participant is marked conflicted; none is runnable/claimable through that relation. | A later valid release/end can leave one remaining participant active. |

Safety-sensitive singleton state never uses signed time or fact ID to select a value. Presentation
labels with multiple maxima render their sorted candidates and conflict state. Peer blocking,
capability revoke, human-device revoke, archive, rejection, retirement, resource exclusion, and
assignment cardinality therefore fail closed.

Specific rules include:

- mailbox actions remain authorized after a revoke only when `action <u revoke`, normally through
  a receiver-signed observation; a concurrent or later action is unauthorized;
- a new mailbox grant after revoke is a new authority root and must descend from the revoke;
- device membership needs a creator grant plus a matching target-key acceptance; a concurrent or
  later revoke wins, and reacceptance is effective only on a regrant path descending from every
  maximal revoke;
- archive loses to a later restore that descends from every maximal archive, but wins concurrently;
- rejection and agent retirement are absorbing removals;
- project histories are home-linear, while active path claims and agent assignments additionally
  obey global cardinality across projects on the same home; and
- activity snapshots use their semantic source sequence rule, while completed item collisions are
  explicit and never resolved by display order.

## Explicit historical authority

Authority is evaluated for the action's authored causal point against the complete known fact set.
The current projection is not authority evidence.

For a mailbox action `a` citing grant `g`:

1. `g` must be a usable grant for the exact target mailbox, grantee installation, and grantee key.
2. `g` must be present in `parents(a)` under the mailbox-grant authority role.
3. The action signer/address/audience must match the grant.
4. For every usable revoke of `g`, `a` remains authorized only if `a <u revoke`. Otherwise the
   revoke is concurrent with or before `a` and the action is unauthorized.
5. A later grant does not retroactively authorize an action that cited `g`; actions after regrant
   cite the new grant.

For an account action, each author cites either the unique creator root or one causal-maximal active
device acceptance for the named account. The cited acceptance must match the exact grant payload
and signer key. Every maximal revoke for that device is considered; selecting one old acceptance
cannot bypass another concurrent revoke.

Installation-private control requires the local installation signer and the exact local subject.
Project transitions require the immutable home signer and previous project head. Remote project
commands require an active human membership authority, but a valid command only requests work; it
cannot itself create project state or runtime success.

## Projection support and retraction

Canonical and control knowledge is add-only. A projection row/value records the exact supporting
fact IDs and, for derived conflict/absence states, the relevant participant IDs. Support is
transitive only through usable dependencies and typed authority roles.

When `E` grows, projection values may:

- appear when missing dependencies arrive;
- disappear when a revoke, block, reject, archive, retirement, conflicting root, project fork, or
  global cardinality conflict becomes known;
- reappear when a valid later restore/regrant/reaccept/release descends from the required frontier;
  or
- change from a singleton to an explicit multivalue/conflicted state.

Retraction removes or changes only rebuildable meaning. It never deletes exact fact bytes, durable
control audit, mutation receipts, outbox lineage, or operational saga checkpoints. A projected
historical fact can remain in audit output while no longer supporting an active view.

Incomplete addressed message observations are a deliberately separate query channel. They expose
validated address/body metadata with `incomplete_causal_history=true`, but they do not create a
thread answer, unread/action support, delivery authority, final-answer selection, or project
dispatch until the fact is projected.

## Canonical presentation comparator

One reducer-owned comparator orders all projected conversation messages and activity, plus the
separate incomplete addressed observations when a query requests them. Storage maintains an index
or cursor derived from this order; clients never recreate it.

The comparator is a deterministic topological Kahn traversal:

1. Build the induced selected-entry graph. A known selected parent contributes an edge. A missing
   parent on an incomplete observation is recorded but cannot become an invented vertex.
2. Place every zero-indegree entry in a ready set.
3. Select the smallest ready key below, emit it, then release its children.
4. Continue until the selected set is exhausted. A remaining cycle is invalid input, never a
   fallback sort.

The ready key is:

```text
(
  authored_at,
  presentation_occurrence,
  family_rank,                 # message before activity on an exact tie
  source_installation,
  source_mailbox,
  provider,
  session,
  operation,
  item_or_request,
  runtime,
  source_sequence,
  stable_public_id,
  fact_id
)
```

For a message, `presentation_occurrence = authored_at` and absent correlation fields are empty
typed values. For activity, it is the bounded signed occurrence time and the remaining correlation,
runtime, and positive sequence values are populated. The stable public ID precedes fact ID only
when the semantic fact family defines one. All components have total domain orderings.

Topological readiness always places a parent before a child even if clocks move backwards.
Occurrence time, authored time, and fact ID affect presentation only. They never grant authority,
resolve a multivalue register, select a project branch, activate membership, or win a domain
conflict.

## Complete-batch reduction

The Complete-batch reduction algorithm is the executable definition of `R`:

1. Deduplicate exact items by typed ID and report any unequal-content collision.
2. Dispatch protocol and semantic variants; retain opaque verified unsupported items.
3. Perform intrinsic semantic, scope, signer/address, parent-role, and graph validation.
4. Build parent/reverse-parent, aggregate, authority-subject, thread, activity-key, project-head,
   path-claim, assignment, and remote-command indexes.
5. Detect present cycles and unique-root/identity collisions.
6. In deterministic dependency order, classify missing/unusable dependencies and evaluate exact
   historical authority against the complete set.
7. Apply every aggregate conflict rule and global cardinality constraint to a fixed point. No map
   iteration or arrival order is observable.
8. Derive frontiers, projection support, normalized aggregates, conflict states, and the canonical
   presentation order.
9. Emit one Normalized reduction report with every collection sorted by its semantic key.

The implementation may optimize these steps, but its public output must equal this definition.

## Incremental equality

Let `delta` be newly known items and `affected(E, delta)` include:

- `delta` and all reverse causal dependants;
- every aggregate sharing a unique root, register subject, authority grant/device, thread,
  activity key, project, path-overlap namespace, agent assignment, or command identity with that
  closure; and
- the reverse dependants of every decision changed while recomputing those aggregates.

Patching projections and decisions for that closure must satisfy:

```text
patch(R(E), reduce_affected(E union delta, affected(E, delta)))
  = R(E union delta)
```

Equality covers decisions and reason codes, blocker sets, frontiers, support IDs, aggregates,
conflicts, presentation order, and normalized observations. Performance never weakens this scope.

## Rust framework mapping

`hq-reducer::reduce_complete` is the single pure complete-batch entry point. `FactSet` owns exact
deduplication and absorbing identity-collision detection; `CausalGraph` owns declared parent and
reverse-dependant indexes, iterative reachability, cycle membership, and affected-descendant
closure. The generic `DomainReducer` interface supplies only closed domain decisions, typed
aggregate membership, typed projection contributions, conflicts, and selected presentation
entries over an immutable `ReductionContext`.

The framework repeats domain classification over normalized complete-set snapshots until the
decision map stabilizes, then derives usable frontiers, transitive usable support, conflicts, and
presentation order. An oscillating domain implementation fails explicitly instead of exposing a
partial or iteration-order-dependent report. `GraphOnlyReducer` is the permissive composition and
graph-law stage; it grants no product authority and derives no domain projection.

## Normalized reduction report

`R` returns representation-independent values suitable for tests and application queries:

```text
ReductionReport {
  decisions: sorted map<KnowledgeId, FactDecision>,
  dependencies: sorted parent and reverse-parent edges,
  frontiers: sorted map<AggregateKey, set<KnowledgeId>>,
  support: sorted map<ProjectionKey, set<KnowledgeId>>,
  conflicts: sorted set<ConflictObservation>,
  identities, mailboxes, peers, capabilities, accounts,
  conversations, activities, agents, sessions, projects,
  remote_commands,
  presentation_order: list<KnowledgeId>
}
```

Normalized output contains typed values, stable IDs, causal relations, state enums, and semantic
timestamps. It excludes exact JSON, signatures, ciphertext, SQL keys, receipt time, relay URL
observations, local row order, UI prose, process state, and filesystem observations. Later
conformance v1 assigns an encoding to these observations without changing their meaning.

## The nine named laws

The acceptance catalog uses these stable law names:

1. `LAW-MERGE-SET-UNION`: component knowledge merges by set union, which is commutative,
   associative, idempotent, and has the empty identity.
2. `LAW-INPUT-INVARIANCE`: permutation, batching, and exact duplication do not change `R`.
3. `LAW-INCREMENTAL-BATCH-EQUALITY`: every incremental patch is exactly equal to fresh complete
   reduction.
4. `LAW-CAUSAL-DOMINANCE`: only usable causality permits semantic dominance; clocks and arrival do
   not.
5. `LAW-EXACT-MAXIMAL-FRONTIERS`: every frontier contains all and only usable causal maxima.
6. `LAW-DEFERRED-READINESS`: missing/unusable dependencies defer support and trigger complete
   affected reconsideration when their decisions change.
7. `LAW-HISTORICAL-AUTHORITY`: typed authority is explicit and evaluated at the action's causal
   point against revocation history.
8. `LAW-PROJECTION-RETRACTION`: knowledge grows monotonically while supported projections may
   retract or become conflicted.
9. `LAW-DETERMINISTIC-CONFLICTS`: every concurrent domain conflict follows the cataloged rule and
   normalized output is identical for every arrival schedule.
