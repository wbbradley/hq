# HQ Rust causal acceptance scenarios

Status: normative semantic acceptance catalog

These scenarios are language- and representation-independent. The Rust testkit will encode them as
deterministic builders, generated DAG/state-machine properties, normalized snapshots, and later
conformance-trace v1 fixtures. Go tests may supply an attack or crash shape, but the expected result
below comes from `docs/rust/causal-algebra.md` and
`docs/rust/semantic-fact-catalog.md`, under the product decisions in
`docs/rust/behavior-ledger.md`.

## Fixture vocabulary

Fixtures name installations `home`, `peer`, and `device`; their signer keys are distinct and
deterministic. Full mailbox addresses always contain installation and mailbox IDs. Accounts,
messages, projects, resources, assignments, provider sessions, commands, timestamps, and random
bytes are supplied explicitly.

Notation:

- `a -> b` means `b` declares `a` as a required parent;
- `a || b` means neither is usefully reachable from the other;
- `authority(role: a)` means `a` is both a parent and the exact typed authority;
- `arrive[...]` is only an ingest schedule and never semantic order;
- `batch(E)` is complete reduction of the set;
- `incremental(schedule)` applies affected-closure updates after each arrival; and
- `observe(name)` selects a normalized field from the report, never a SQL row or wire DTO.

Every example is run as one complete batch, every relevant prefix, forward and reverse one-at-a-time
arrival, a seeded permutation sample, and schedules with exact duplicates. Small DAGs enumerate all
topological orders. Property generators expand the same laws to bounded arbitrary DAGs.

## Required normalized evidence

Each scenario records, as applicable:

- per-item decision and closed reason code;
- missing and present-unusable blockers;
- parent/reverse-parent edges and usable causal relations;
- exact aggregate frontiers and conflict participants;
- projection keys and complete support-ID sets;
- identity, route, capability, membership, conversation, activity, agent/session, project, claim,
  assignment, dispatch, and remote-command views;
- canonical presentation order and incomplete-history flags; and
- equality of batch, incremental, and repair-normalized reports.

Untrusted bytes, exact protocol encoding, crypto vectors, SQLite state, process/runtime state, and
relay wrappers have later acceptance catalogs. These scenarios can mention their normalized trust
outcome without specifying those representations.

## Scenario matrix

| ID | Name | Given | When | Expected normalized result | Requirement evidence |
| --- | --- | --- | --- | --- | --- |
| LAW-001 | LAW-MERGE-SET-UNION | Three finite fact sets with overlaps and empty set | Merge them in both orders, associations, duplicate/self, and empty combinations | Typed knowledge sets are equal and each ID occurs once | Algebra law 1 |
| LAW-002 | LAW-INPUT-INVARIANCE | One valid mixed-domain fact set | Reduce all permutations, batch partitions, and duplicate schedules | Entire normalized report is byte-for-byte equal after conformance encoding | Algebra law 2 |
| LAW-003 | LAW-INCREMENTAL-BATCH-EQUALITY | Generated DAGs including late parents, revokes, global project conflicts, and duplicates | Compare every incremental prefix and repaired store with fresh batch | Decisions, blockers, frontiers, support, aggregates, conflicts, and order are exactly equal | Algebra law 3 |
| LAW-004 | LAW-CAUSAL-DOMINANCE | Causal child with earlier authored time and concurrent fact with later authored time | Reduce and compare dominance | Child may dominate its ancestor; concurrent fact uses domain rule; neither clock nor arrival decides | Algebra law 4 |
| LAW-005 | LAW-EXACT-MAXIMAL-FRONTIERS | Diamond DAG plus unrelated and conflicted aggregates | Compute each aggregate frontier | Every usable maximum and only those maxima appear; unrelated descendants change nothing | Algebra law 5 |
| LAW-006 | LAW-DEFERRED-READINESS | Child arrives before one parent and has a reverse-dependent grandchild | Add the missing parent last | Child/grandchild move from unresolved only after full usability and affected closure is reconsidered | Algebra law 6 |
| LAW-007 | LAW-HISTORICAL-AUTHORITY | Grant, action, observation, revoke, regrant, and actions citing old/new grants | Reduce every relevant causal arrangement | Exact causal authority determines each action; current route/grant display never substitutes | Algebra law 7 |
| LAW-008 | LAW-PROJECTION-RETRACTION | Initially projected action/selection/claim later receives revoke/conflict | Grow the fact set | Knowledge only grows while active projections retract and support/conflict evidence remains | Algebra law 8 |
| LAW-009 | LAW-DETERMINISTIC-CONFLICTS | One instance of every conflict pattern | Reduce every arrival schedule | Each result follows its catalog rule with no timestamp, map-order, or identifier winner | Algebra law 9 |
| GRAPH-001 | missing-parent-chain | Grandchild cites absent child which cites absent root | Add grandchild, child, root in reverse | Missing blocker sets shrink deterministically; no descendant projects early | Required dependency safety |
| GRAPH-002 | present-unusable-parent | Child cites a present unauthorized fact | Add an unrelated valid authority later | Child stays unresolved through the unusable parent; unrelated authority changes nothing | Usable reachability |
| GRAPH-003 | self-parent-invalid | A supported fact cites itself | Reduce | Fact is invalid with self-cycle reason and supports nothing | Graph validity |
| GRAPH-004 | present-cycle-invalid | Three present supported facts form a declared cycle fixture | Reduce | Cycle participants are invalid and every dependant is unresolved by unusable parent | Graph validity |
| GRAPH-005 | duplicate-exact-input | Identical exact verified item appears repeatedly and in several batches | Reduce | One knowledge item and duplicate ingest observations; no semantic change | Deduplication |
| GRAPH-006 | typed-id-protocol-separation | Canonical and remote-control records have equal raw digest bytes in their own namespaces | Reduce | Typed IDs remain distinct and cross-protocol role validation is explicit | Protocol ownership |
| GRAPH-007 | unsupported-parent-blocks | Supported child cites an opaque verified unsupported parent | Reduce before and after unrelated supported facts | Parent is unsupported and child unresolved by present-unusable parent | Trust transition |
| GRAPH-008 | conflicting-unique-root | Two unequal roots claim one account/project/mailbox identity | Reduce in all orders | Roots and aggregate are conflicted; no chosen active root | Unique-root safety |
| AUTH-001 | local-control-wrong-signer | Peer/agent/account-selection fact is signed by another installation | Reduce | Invalid or unauthorized by exact local-control reason; no local projection | Installation authority |
| AUTH-002 | unrelated-parent-authority-attack | Peer action has a valid unrelated descendant of a grant but omits typed grant authority | Reduce | Action is unauthorized; causal proximity is not authority | Explicit authority attack |
| AUTH-003 | directional-mailbox-grant | A grants B access to mailbox on A | B sends to that mailbox and A tries using the grant in reverse | B action projects; reverse-direction action is unauthorized | Directional capability |
| AUTH-004 | peer-route-is-not-capability | Valid peer route exists without mailbox grant | Peer sends a message | Message is unauthorized although routing identity is known | Route/authority separation |
| AUTH-005 | observed-pre-revoke-action | Grant -> action -> owner observation -> revoke | Add revoke last | Action remains projected and observation/revoke frontier is exact | Historical authority |
| AUTH-006 | concurrent-revoke-action | Grant has concurrent action and revoke without observation path | Reduce all schedules | Action is unauthorized and any prior projection retracts | Remove-wins capability |
| AUTH-007 | post-revoke-old-grant | Action descends from revoke but cites the revoked grant | Reduce | Action is unauthorized | Historical authority |
| AUTH-008 | regrant-descends-revoke | New grant descends from all revoke maxima and action cites it | Reduce | New grant/action project; old-grant actions remain unauthorized | Capability restoration |
| AUTH-009 | partial-regrant-frontier | Two concurrent revokes exist and regrant descends from only one | Reduce | Regrant/action remain unresolved or conflicted; access is inactive | Maximal frontier safety |
| AUTH-010 | observation-frontier-minimality | Long chain and concurrent branches of owner observations | Reduce | Observation frontier contains every and only maximal observations | Frontier law |
| AUTH-011 | peer-block-concurrent-set | Peer route set and block are concurrent | Reduce | Route is blocked and prior authorized facts remain visible | Remove-wins routing |
| AUTH-012 | peer-route-restored-after-block | Route set descends from every maximal block | Reduce | Route becomes singular/routable without changing history | Explicit restoration |
| AUTH-013 | account-grant-accept | Creator root, exact grant, and target-key acceptance | Reduce | Device active with grant/accept support and exact frontier | Human membership |
| AUTH-014 | accept-without-grant | Target signs matching-looking acceptance but grant is absent | Reduce | Acceptance unresolved and device inactive | Human membership dependency |
| AUTH-015 | accept-changed-payload-or-key | Acceptance changes label/key/relay or uses another signer | Reduce | Invalid/unauthorized exact-match reason; device inactive | Human membership binding |
| AUTH-016 | concurrent-device-accept-revoke | Acceptance and creator revoke share grant but are concurrent | Reduce | Revoke wins and device inactive | Human remove-wins |
| AUTH-017 | post-revoke-reaccept-old-grant | Acceptance descends from revoke but cites old grant | Reduce | Device inactive; acceptance unauthorized or conflicted | Human regrant safety |
| AUTH-018 | regrant-reaccept-all-maxima | New grant and acceptance descend from every revoke maximum | Reduce | Device active through new causal-maximal acceptance | Human restoration |
| AUTH-019 | wrong-account-membership | Action for account X cites valid membership in Y | Reduce | Unauthorized for named account X | Audience binding |
| AUTH-020 | account-selection-arbitrary-descendant | Local selection cites non-membership descendant of account history | Reduce | No default account projection | Typed membership role |
| AUTH-021 | conflicting-account-root | Two creators claim one account ID | Reduce | Account/root facts conflicted and no device authority exists | Unique-root safety |
| AUTH-022 | revoked-source-activity | Revoked device authors account activity after revoke | Ingest valid encrypted/signed semantic activity | Activity unauthorized and no activity/conversation projection | Account activity authority |
| CONV-001 | local-question-answer | Agent asks local human and human answers citing root | Reduce | One thread, question then answer, ready answer eligible | Messaging baseline |
| CONV-002 | multiple-answers | Several valid answers are concurrent/causal | Reduce | All answers retained; wait order is canonical presentation order, not arrival | Answer accumulation |
| CONV-003 | answer-cancel-before-after-concurrent | One answer before, one after, and one concurrent with cancellations | Reduce | Relation matrix reports each exact causal relation; answered and cancelled both true | Independent facts |
| CONV-004 | missing-question-observation | Addressed answer arrives before question root | Query direct observation and reduce | Incomplete content may be observed, but no answered thread/action/final support exists | Deferred visibility |
| CONV-005 | message-public-id-collision | Unequal message facts reuse one stable public message ID | Reduce | Collision is explicit and neither ambiguous handle drives action | Stable identity safety |
| CONV-006 | state-target-not-ancestor | Archive cites unrelated message as target but not causal ancestor | Reduce | Archive invalid and target remains open | Target causality |
| CONV-007 | archive-restore | Message -> archive -> frontier-complete restore | Reduce | Message open, facts retained, support points to restore and target | Restore semantics |
| CONV-008 | archive-concurrent-restore | Archive and restore are concurrent maxima | Reduce | Archive wins and message closed | Remove-wins archive |
| CONV-009 | rearchive-after-restore | Later archive descends from restore | Reduce | Message archived again with exact frontier/support | Reversible state |
| CONV-010 | reject-absorbing | Valid reject plus concurrent or later restore | Reduce | Rejected and closed permanently; restore cannot reopen | Absorbing rejection |
| CONV-011 | typed-semantics-ignore-prose | Body/details imitate authority, final answer, correlation, and technical labels | Reduce | Only typed fields affect projections; prose remains display content | No behavioral parsing |
| CONV-012 | peer-received-causal-proof | Valid child action from peer cites an outbound parent | Reduce | Parent delivery view may show peer received; relay acceptance alone cannot | Delivery semantics |
| CONV-013 | account-fanout-one-fact | Same account fact arrives through wrappers for several devices | Reduce on each device | Same fact ID/decision/conversation meaning; device-local delivery presentation may differ | Replica convergence |
| CONV-014 | final-answer-selection | Several output entries include typed updates and final-answer candidates in one action group | Reduce | Every entry remains; presentation query selects the canonical-order terminal candidate by typed group | Typed presentation |
| CONV-015 | equal-time-mixed-order | Concurrent messages/activity share authored and occurrence times | Reduce permutations | Sole comparator yields one stable total presentation order | Canonical ordering |
| CONV-016 | child-clock-before-parent | Child authored time precedes parent time | Reduce | Parent is emitted first by topology | Causal presentation |
| ACT-001 | activity-message-inertness | Activity surrounds question/answer/archive facts | Reduce | Activity changes no inbox, unread, action, reply, archive, delivery, draft, or final-answer state | Dual-stream separation |
| ACT-002 | sequenced-plan-snapshots | Causal plan snapshots have increasing source sequence | Reduce reverse arrival | Highest causal sequence is selected and exact facts remain | Snapshot coalescing |
| ACT-003 | concurrent-sequence-winner | Same source/runtime logical key has concurrent unequal sequence snapshots | Reduce | Higher semantic sequence wins; decision is independent of display comparator | Explicit activity rule |
| ACT-004 | equal-sequence-collision | Same source/runtime/key/sequence has unequal payloads | Reduce | Explicit activity collision; no hidden fact-ID winner | Activity safety |
| ACT-005 | cross-provider-namespace | Equal session/operation/item strings from two providers | Reduce | Separate activity keys/projections | Namespace isolation |
| ACT-006 | cross-mailbox-source-namespace | Equal provider correlation from two source mailboxes | Reduce | Separate activity keys/projections | Source identity isolation |
| ACT-007 | delayed-occurrence-data | Activity occurrence time is far earlier/later than authored time | Reduce | Topology and authored-time ready key bound placement; occurrence affects presentation tie only | Clock containment |
| ACT-008 | completed-item-versus-progress | Progress and completed item share operation/item correlation | Reduce | Completed semantic item remains durable history; replaceable progress stays non-actionable | Activity retention |
| ACT-009 | compacted-view-rebuild | More progress keys than retained budget plus superseded snapshots | Batch, incremental, and repair | Exact facts retained; compacted winner/budget view is deterministic and equal | Projection retention |
| AGT-001 | unique-agent-name | One name claim for one local agent mailbox | Reduce | Active named agent and permanent reservation | Named agents |
| AGT-002 | concurrent-name-claims | Two mailboxes claim one name concurrently | Reduce | Name conflicted/reserved and neither is runnable under it | Unique identity conflict |
| AGT-003 | session-binding-reassignment | Same provider/session is bound to two mailboxes | Reduce | Binding conflict; no silent reassignment | Session identity |
| AGT-004 | concurrent-session-selection | Two different sessions are selected at one agent frontier | Reduce | Both maxima exposed and runnable selection blocked | Multivalue register |
| AGT-005 | resolve-session-selection | Later selection cites every conflicting selection maximum | Reduce | One durable selected session | Frontier resolution |
| AGT-006 | concurrent-session-renames | Unequal names for one session are concurrent | Reduce | Sorted candidates/conflict exposed; selection/runtime unchanged | Display register |
| AGT-007 | retirement-concurrent-selection | Retirement and session selection are concurrent | Reduce | Retirement wins; session remains historical and worker cannot launch | Absorbing retirement |
| AGT-008 | retired-name-reuse | New mailbox claims a retired name | Reduce | Conflict/rejection; reservation is permanent | Named-agent safety |
| AGT-009 | context-history-multivalue | Session/mailbox records concurrent repository contexts | Reduce | All history/frontier values retained; context grants no authority | Context semantics |
| AGT-010 | cross-provider-session-namespace | Same external session text in two providers | Reduce | Distinct session identities and histories | Provider isolation |
| PRJ-001 | project-create-closed | Home creates a closed project with resources and primary path | Reduce | Unique project/head, desired resources, no active claims/assignment | Project baseline |
| PRJ-002 | project-create-open-claims | Home creates open project with conflict-free resources | Reduce | Open with one active claim per desired resource | Claim model |
| PRJ-003 | project-linear-fork | Two different home-signed children cite one project head | Reduce all orders | Project stops at common head and exposes both fork participants | Linear history safety |
| PRJ-004 | stale-head-transition | Transition cites an ancestor instead of current unique head | Reduce | Conflicted/invalid stale branch; authoritative head unchanged | Expected-head rule |
| PRJ-005 | cross-project-resource-conflict | Two open projects on same home claim equal or ancestor/descendant paths | Reduce | All overlapping claims/projects conflicted and non-runnable; no timestamp winner | Global path safety |
| PRJ-006 | project-local-overlap | One project claims parent and child paths | Reduce | Both desired/active claims allowed | Path policy |
| PRJ-007 | different-home-same-path | Projects on different homes use same path spelling | Reduce | No resource conflict because locator namespaces differ | Home-qualified identity |
| PRJ-008 | reopen-atomic-conflict | Closed project reopening has one conflicting desired resource | Reduce | Remains closed with no partial active claims | Atomic claims |
| PRJ-009 | close-releases-without-delete | Open assigned project moves closing -> assignment end -> closed | Reduce | Claims/assignment released, files absent from semantics, all history retained | Close invariant |
| PRJ-010 | archive-requires-closed | Archive fact follows open head without close | Reduce | Invalid transition; open state unchanged | Lifecycle model |
| PRJ-011 | unarchive-remains-closed | Archived project receives valid unarchive | Reduce | Visible but closed, unassigned, and claim-free | Archive model |
| PRJ-012 | double-agent-assignment | Same agent is assigned by two open projects on one home | Reduce | Both competing assignments non-runnable and conflict visible | Global cardinality |
| PRJ-013 | assignment-resolution-by-end | One of two conflicting assignment epochs ends validly | Reduce | Remaining unique assignment may become active/runnable if otherwise supported | Projection retraction |
| PRJ-014 | thread-scope-mismatch | Runnable assignment cites thread for another agent/project | Reduce | Fact invalid/unresolved and assignment stays configuring | Thread immutability |
| PRJ-015 | activation-failure-compensation | Preparing/configuring facts are followed by documented abort/end path | Reduce | Prior closed project returns closed or prior open remains open; no stranded configuring state | Activation saga semantics |
| PRJ-016 | input-sequence-contiguous | Home accepts several project messages | Reduce reorder | Sequence is unique/contiguous in home history and pending order stable | Input sequencing |
| PRJ-017 | dispatch-at-most-once | Two dispatch facts claim same input or sequence | Reduce | Explicit conflict and no duplicate current dispatch | Delivery safety |
| PRJ-018 | dispatch-attribution | Acceptance followed by runnable assignment and exact dispatch | Reduce | Immutable project/assignment/agent/thread provenance | Auditability |
| PRJ-019 | late-output-after-handoff | Old assignment emits output after end/new assignment | Reduce | Output retained and marked late; cannot mutate project or appear current | Late output safety |
| PRJ-020 | stable-output-collision | Same output ID retries with changed presentation/correlation/body/provenance | Reduce | Changed retry conflicts; identical retry deduplicates | Output idempotency |
| PRJ-021 | forced-close-observation | Forced close records runtime unknown/still-running | Reduce | Project closed/claims released but no external-cessation claim exists | Advisory boundary |
| PRJ-022 | resource-health-no-lifecycle | Health changes healthy -> missing/inaccessible -> recovered | Reduce | Health/notices change; lifecycle, assignment, and claims do not automatically change | Observation boundary |
| PRJ-023 | resource-replacement-atomic | Open project replaces primary resource after external saga reconciliation | Reduce | Old claim ends, new claim starts, primary follows, or whole transition fails closed | Resource transition |
| CTL-001 | remote-command-queued-only | Active device authors valid remote request while home offline | Reduce | Command accepted/queued; project state unchanged | Remote-control isolation |
| CTL-002 | remote-command-receipt | Home signs receipt for exact request and observed head | Reduce | Stage received with head; project state unchanged | Remote-control stages |
| CTL-003 | remote-command-committed | Home emits canonical transition then signed committed outcome citing it | Reduce | Project advances only through canonical fact; command terminal view cites new head | Home authority |
| CTL-004 | remote-command-rejected-stale | Request expected old head and home rejects with current head | Reduce | Project unchanged; command rejected with stale-head typed result | Expected-head safety |
| CTL-005 | same-id-different-input | Two remote requests reuse command ID with different digest/body | Reduce | Command identity conflict; neither changed request is treated as replay | Idempotency collision |
| CTL-006 | conflicting-terminal-outcomes | Home key signs unequal terminal outcomes for one request | Reduce | Explicit terminal conflict and no selected committed/rejected result | Control integrity |
| CTL-007 | relay-acceptance-not-receipt | Relay accepts wrapper but no home receipt exists | Query command state | Remains queued/relayed operationally, not semantically received | Transport separation |
| CTL-008 | runtime-uncertain-outcome | Home receives runtime command but external effect cannot be reconciled | Reduce outcome | Typed uncertainty visible; no false committed success | External boundary |
| SEC-001 | signer-address-mismatch | Valid signature key differs from semantic author installation/address | Reduce | Invalid/unauthorized and no projection | Identity binding |
| SEC-002 | audience-mismatch | Account action cites active membership but names another audience | Reduce | Unauthorized for exact account | Audience binding |
| SEC-003 | diagnostic-authority-injection | Body/details/technical fields contain valid IDs and authority-like text | Reduce | No authority or state effect beyond inert display | No prose parsing |
| SEC-004 | receiver-clock-invariance | Same facts are ingested with different receipt clocks | Reduce | Reports equal except excluded ingestion diagnostics | Clock independence |
| SEC-005 | relay-order-invariance | Same encrypted logical facts are observed in different wrapper/relay order | Reduce semantic facts | Reports equal; relay observations absent from reducer | Transport independence |
| SEC-006 | unusable-bridge-attack | Unauthorized intermediary links valid grant to attacker child | Reduce | Intermediary unusable and child unresolved/unauthorized; no usable reachability | Authority laundering defense |
| REG-001 | REG-AUTHORITY-MAXIMAL-REGRANT | Revoke plus several historical/concurrent acceptances and partial/full regrant branches | Reduce all topological schedules | Only acceptance on regrant descending every maximal revoke grants authority | Former Go regression |
| REG-002 | REG-CONVERSATION-COMPARATOR | Equal-time mixed messages/activity with late parent and delayed occurrence | Batch, incremental, page, rebuild, and UI-normalized consumption | One parent-first total order everywhere; no local lookalike sort | Former Go regression |
| REG-003 | REG-INDEXED-PAGINATION | Large conversation with equal-time mixed entries and stable page limit | Concatenate all cursor pages and measure later-page work | Concatenation equals reducer order and later pages avoid full-history load/sort | Former Go regression |
| REG-004 | REG-NONDISRUPTIVE-RELAY-WAKE | Healthy live subscription receives many publish/config work wakes | Script relay session | Work coalesces promptly without subscription/session restart; ordinary transport cannot affect reduction | Former Go regression |

## Generated property suites

The named examples seed generators rather than replacing them:

- DAG generation varies roots, fanout, missing parents, unusable bridges, aggregate membership,
  duplicate batches, authored times, and every topological arrival schedule for small graphs.
- Authority generation varies action/observation/revoke/regrant edges and all maximal acceptance
  subsets. It asserts fail-closed outcomes for omitted or unrelated typed authorities.
- Conversation generation varies answers, cancellations, archive/restore/reject frontiers,
  activity keys/sequences, equal times, delayed occurrence, and page boundaries.
- Agent generation varies name/binding/selection/rename/retirement frontiers and provider
  namespaces.
- Project state-machine generation produces only commands from a pure transition model, then also
  injects stale heads, home forks, path overlap, double assignment, dispatch collision, late output,
  and invalid scope. Every prefix is reduced from scratch and incrementally.
- Remote-control generation varies request replay/digest collision, home receipt, stale head,
  terminal result, loss/reorder, and explicit runtime uncertainty while asserting that only
  canonical home facts mutate project state.

Shrunk counterexamples must print the semantic fact list, parent/authority roles, arrival schedule,
expected versus actual normalized report, and the first differing path. No property relies on
sleeps, ambient clocks/randomness, public relays, installed providers, or Go output.

## Storage query work gates

- Conversation page limits are exactly `1..=200`; storage fetches no more than `limit + 1` order
  rows and hydrates no more than `limit` typed projection values.
- Cursor anchor lookup and later-page selection use the primary/unique conversation-local covering
  indexes. A page performs no canonical-event scan, complete projection load, or in-memory sort.
- `REG-INDEXED-PAGINATION` uses at least 1,000 equal-time mixed message/activity entries, walks the
  entire result with non-dividing page sizes, compares concatenation to reducer order, and inspects
  the SQLite query plan for indexed range selection.
- Incremental write tests protect unrelated projection rows with aborting update/delete triggers;
  a semantically unrelated append must commit without firing them, then equal batch and repair.

## Application service gates

- Exact fact-mutation replay returns the retained typed receipt without invoking the pure decision
  again; the same command ID with a changed digest conflicts before decision.
- A committed receipt remains committed when its post-commit publish wake is coalesced or fails.
  Rejected and uncertain attempts schedule no publish work.
- Relay, synchronization, neutral session, and resource operations carry stable operation IDs and
  exact digests and expose accepted, rejected, or reconcilable uncertain outcomes.
- Subscription preparation traces `register -> authoritative query`; activation is a separate call
  after acknowledgement, while query failure traces `register -> query -> cancel`.
- The store gateway returns revision plus all four application-owned projection packages from one
  actor request, translates pure fact plans through the ordinary atomic mutation engine, and
  strictly validates retained application result bytes and result-kind agreement.
- Architecture verification rejects runtime, persistence, transport, terminal, filesystem,
  process, and provider-specific implementation concerns in `hq-application`.

## Local API v1 wire gates

- Four-byte big-endian framing rejects an oversized declaration before body decode, and rejects
  truncation, trailing bytes, malformed UTF-8/JSON, unknown fields/variants, and any decodable JSON
  whose canonical re-encoding differs.
- Negotiation chooses the highest common nonzero version, reports disjoint ranges explicitly, and
  carries bounded diagnostic build metadata without using build identity as authority.
- Every lifecycle, query, fact mutation, relay/sync effect, neutral agent-session effect, resource
  inspection, typed project command/progress, subscription, response, error, and invalidation
  family round-trips through the one v1
  codec; decoded values reapply all constructor bounds.
- Every one of the 48 semantic fact families round-trips through the unsigned local planning bridge.
  The exact mutation digest binds canonical plan bytes plus auxiliary randomness; changing either
  under the same command ID changes or invalidates the digest.
- Snapshot and conversation-page values are bounded, typed client DTOs rather than serialized Rust
  domain/application structs or storage rows. Invalidations contain revision/topics/full-refresh
  intent only and never contain projection rows or message bodies.
- Architecture verification allows `hq-local-api` to depend inward on domain, canonical protocol,
  and application only; no storage dependency or SQLite vocabulary is present.
- Server-session contracts require a written hello before requests, only one unconfirmed response,
  session-owned single-use write tickets, post-write subscription activation, and idempotent cleanup
  after lost responses or stale disconnects. Every typed request family routes through application
  capabilities and the separate node-lifecycle capability borrowed only for that call; a session
  retains no concrete node capability, and dropping it cancels only its own revision registrations.
- Revision-hub contracts cover pending/active race phases, unrelated-topic filtering, saturated
  registration, 10,000 coalesced slow-reader publishes, and concurrent publish/poll/cancel without
  blocking or growing more than one pending wake per subscriber.
- Reconnecting-client contracts cover negotiation on every generation, stale-socket rejection,
  capped deterministic backoff, terminal version incompatibility, byte-identical mutation and
  project-command replay across ambiguous pre/post-commit response loss, changed-command rejection,
  bounded completed
  identity history, correlated ordinary response/loss reporting, and clean lifecycle restart.
- Client subscription contracts cover per-server-session registration identities, early and
  coalesced invalidations, acknowledgement snapshots as fresh bases, repeated full refresh while a
  returned revision is behind, resubscription after acknowledgement loss, and two independent
  clients racing without sharing registration state.

## Node lifecycle foundation gates

- Runtime paths are explicit or derive to an installation-qualified XDG namespace with a private
  state-local fallback. Relative/empty roots, symlinks, modes other than `0700`, and socket paths
  beyond the portable 103-byte limit fail before bind work; path preparation never deletes an
  unowned stale socket or readiness artifact.
- Lifecycle contracts cover startup, store-revision readiness, read/query/mutation/launch admission,
  mutation and launch rejection at drain entry, idempotent stop, explicit clean-restart intent,
  out-of-order events, retained failures, and terminal stop acknowledgement without ambient time or
  process state.
- Foundation contracts cover two concurrent state owners, missing identity, unsafe runtime,
  store-open failure, reverse-order startup rollback, checked store close, redacted structured
  component/cause/action diagnostics, and immediate state-lock/store reacquisition after clean or
  failed startup.
- Component-owner contracts fail startup independently at all four acknowledgement positions,
  force-stop a partially started owner, roll earlier owners back in reverse, and release the store
  and state lock for immediate reacquisition.
- Coordination contracts cover parent/child/sibling cancellation, fixed mailbox saturation and
  closure without item loss, task-tracker saturation and closed intake, returned task failure,
  panic joining, and zero retained handles after drain.
- Shutdown traces close lifecycle admission before component intake, cancel the root, drain local/
  relay/harness/project owners in normative order, escalate only explicit or failed drains, join
  every task, and release the foundation. Stop/drain errors accumulate in the final typed report
  without skipping later components or ownership release.
- Unix listener contracts reject reserved symlinks, regular files, unsafe modes, and live
  listeners; replace only an identity-stable connection-refused socket; enforce `0600`; and accept
  protocol bytes only after Linux/macOS kernel credentials match the effective user.
- Readiness contracts cover ready-only versioned records, nonzero process/installation/boot values,
  bounded canonical decode and pre-allocation file limits, private atomic replacement, duplicate
  boot-nonce rejection, and the rule that stale readiness never grants or blocks listener
  ownership.
- Runtime cleanup contracts cover component-start rollback, exact socket/readiness removal,
  substituted path preservation with a typed shutdown issue, continued store/state-lock release,
  no temporary-file leak, and immediate listener rebind.
- Session-I/O contracts cover opaque peer-validated streams, partial and multiple frame reads,
  malformed/oversized/truncated rejection, bounded decoded-event backpressure, fixed encoded-write
  capacity, invalid untracked-message rejection, full-frame ticket completion, cancellation after a
  partial write without completion, exactly one terminal event, and a joined driver with no child
  task.

## Scenario maintenance rule

Every new semantic catalog row adds at least one valid scenario and one missing-parent,
authorization, invalid-shape, conflict, or retention scenario as applicable. Every reducer defect
gets a stable scenario name before its fix. A test may strengthen an expectation, but deleting or
weakening a named scenario requires an explicit specification and behavior-ledger review.
