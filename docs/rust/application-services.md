# Application services and ports

Status: normative first-release application boundary

`hq-application` owns use cases and the semantic values exchanged with adapters. It depends only on
`hq-domain` and `hq-reducer`. It contains no persistence, local framing, relay protocol, terminal,
filesystem, process, runtime, provider-specific, clock, randomness, or signing implementation.
Time, stable identities, request digests, causal references, and signing randomness enter as
explicit typed values.

## Authoritative query values

`ProjectionSnapshot<A, K, V>` owns ordered aggregate frontiers, typed projections, and exact support
sets. The four aliases cover authority/account/peer configuration, conversations and activity,
named agents and session selection, and projects. `DomainSnapshot` contains all four packages.
`AuthoritativeSnapshot` pairs them with the one local `Revision` at which they were read.

These values belong to the consumer boundary, not storage. A persistence adapter may re-export them
and reconstruct them through strict relational codecs, but callers never receive tables, SQL keys,
serialized reducer structs, or a persistence handle. Unified conversation pages contain only the
closed `ConversationEntry::{Message, Activity}` union and retain reducer-derived order and the
store-owned cursor.

## Consumer-owned ports

| Capability | Contract |
| --- | --- |
| `QueryDomain` | Revisioned authoritative refresh and domain health, explicit rebuildable-state repair, bounded indexed conversation pages, and bounded exact causal-evidence closure |
| `CommitFacts` | Execute or reconcile a stable transaction-consistent fact mutation, or reverify and idempotently ingest exact public evidence |
| `PublishWake` | Nonblocking, coalescible prompt for post-commit replication/reconciliation work |
| `ConfigureRelays` | Stable relay-policy and explicit synchronization operations plus bounded durable relay/delivery status |
| `ControlHarness` | Neutral named-agent start, exact resume, and stop operations |
| `ControlProjects` | Local execution or durable routing of one exact project command |
| `RetireAgents` | Route exact named-agent retirement through idle commit or the owning project saga |
| `InspectResource` | Typed external observation without claiming a durable state transition |
| `ObserveRevisions` | Pending registration, later activation, and idempotent cancellation |

Ports name capabilities needed by the application. They do not mirror methods of a database,
socket, relay library, process client, filesystem library, or UI. The node composition root supplies
one `ApplicationPorts` bundle whose implementations may delegate to independently owned adapters.

`ApplicationError` has closed classes and codes. Adapters discard implementation prose and map
failures to invalid input, conflict, unauthorized, unresolved, not found, capacity, unavailable,
corrupt state, or invariant violation. Only later client and UI layers add safe presentation text.

## Fact-backed mutations and replay

`FactMutation` binds a 32-byte command ID to the digest of its exact request and a one-shot pure
decision. A commit adapter invokes that decision against a `DomainSnapshot` read inside the owning
transaction. The decision either rejects with a typed `DomainError` or returns a `FactPlan` made
only from explicit author, time, audience, causal and historical-authority references, semantic
payload, and BIP-340 auxiliary randomness. The protocol/storage adapter translates that plan into
canonical authoring and signing; application code never owns protocol DTOs or signer bytes.

The attempt result is one of:

- `Completed(MutationReceipt)` with an authoritative `Committed` or typed `Rejected` outcome; or
- `Uncertain { command_id, request_digest }` when a response may have been lost and exact replay is
  required before any new attempt.

The application-owned receipt encoding is canonical and versioned. A commit is bytes `01 00`. A
rejection is `01 01`, one closed error-category byte, a two-byte big-endian error-code length, and
the bounded UTF-8 stable error code. Unknown versions/categories, trailing data, invalid UTF-8,
length mismatch, noncanonical re-encoding, or disagreement with the store's committed/rejected kind
is corrupt state. Rust layouts, transport results, diagnostic prose, and secrets are never retained.

After a committed receipt, `Application::execute_mutation` separately calls `PublishWake`. The
returned `MutationCompletion` keeps the receipt authoritative and reports scheduling as
`Scheduled`, `Coalesced`, or a typed error. A wake failure cannot turn a committed command into a
failure and cannot justify retrying it under a new identity. Rejections and uncertain attempts do
not schedule work.

Human-account administration uses pure planners in this crate. Each planner accepts public passive
`LocalInstallationAuthority` and `LocalFactInputs` records and returns an ordinary `FactPlan` for
reserved human-mailbox creation, creator-account creation, or frontier-complete account selection.
It also plans creator-only frontier-complete device grants and revocations plus exact target-key
device acceptance. A revoke request names the permanent creator address, exact grant identity and
fact, target device, and complete membership frontier; the planner rejects a non-creator or creator
self-revoke before returning a plan.
The records expose fields directly; they contain no secret or mutable capability. The CLI supplies
only exact roots and frontiers from an authoritative snapshot, while the node-owned gateway remains
the sole signer and commit capability.

Directional peer and mailbox administration uses separate pure planners. Public passive request
records carry the exact peer address and encryption metadata, complete route frontier, exact local
mailbox creation fact, exact grantee address, stable grant identity, and complete revoke or
capability lineage. Route set/block plans bind local-installation authority; mailbox grant/revoke
plans bind the exact mailbox-owner or mailbox-grant authority. The planners reject self-routes,
self-grants, nonlocal mailbox ownership, oversized support, and incomplete typed construction before
returning an ordinary `FactPlan`. Route trust remains directional and distinct from mailbox access;
relay hints and encryption keys never become authority.

Named-agent catalog administration likewise uses pure planners with public passive records. Agent
mailbox creation and permanent name claims bind the local installation root and exact projected
agent-mailbox root. Durable session selection additionally cites the exact compatible name claim,
immutable binding, matching typed context fact, and complete prior selection frontier. Display
rename/clear cites the exact claim and binding plus the complete independent rename frontier; it
does not select or start a runtime. The authoritative client projection exposes the required claim,
mailbox, binding, candidate, and frontier evidence directly. Because HQ has not shipped, local API
v1 was evolved in place without compatibility accessors or a version bump.

Retirement planning is also pure: its passive request carries the exact claim, mailbox, and
complete agent-selection frontier and yields one installation-private `AgentRetired` plan. The
behavioral `RetireAgents` capability is deliberately separate. Its coordinator rechecks the active
human and global assignment set, commits an idle retirement transactionally, or delegates an
assigned agent to the one owning project saga. A stale claim, fork, wrong home, changed command
identity, or multiple assignment fails closed. No caller may author retirement directly from an
external stop result.

## External operations

`EffectRequest<T>` carries a stable `OperationId`, exact request digest, explicit issue time, and
typed body. Relay configuration, synchronization, neutral agent-session control, and resource
inspection each retain their own capability trait because their reconciliation rules differ. Their
common `EffectOutcome<T>` is `Accepted`, typed `Rejected`, or `Uncertain(operation_id)`. Uncertainty
means reconcile that identity before repeating the effect; persisted intent alone is never accepted
evidence that external work happened.

Relay configuration contains only a typed endpoint locator, read/write policy, authentication
policy, and enabled state. It contains no credentials or client-library values. Relay status is a
passive bounded record of at most 256 current policies and durable delivery-state counts, including
an explicit truncation bit. `StateHealth` and `StateRepairReport` expose stable ordered decision and
conflict counts for all four reducer domains; repair carries the caller's stable operation identity.
Session control names a durable agent,
neutral provider namespace, and start/exact-resume/stop action. Resource inspection names the
project, resource, display locator, and recorded canonical locator. It returns only bounded inert
details, typed health, an optional newly observed canonical locator, and an explicit observation
time. These passive request/result values expose fields directly. Observation alone grants no
project authority; workflow owners turn accepted observations into canonical decisions.

## Project lifecycle workflows

`hq-projects` composes five narrow capabilities: durable saga checkpoints, transaction-consistent
canonical project compare-and-swap, read-only resource observation, project-bound runtime control,
and a separate bounded mutating Git worktree capability. Passive snapshots, mutation requests,
resource reports, runtime requests, Git requests/results, and delivery records expose public fields.
The managers remain opaque because they own checkpoint order, bounded recovery, compensation, and
exact retry.

A configuring assignment contains only assignment, agent, and provider intent. A fresh provider
session cannot be known before runtime readiness, so the exact session is bound only by the
canonical runnable transition. Activation validates expected head, immutable home, active human,
claimability, agent cardinality, desired resources, and launch directory around the external start
or exact-resume boundary. A definite failure ends the configuring assignment and restores a
workflow-opened project to closed; uncertainty retains the original typed failure and resumes by
stable identity. The exact pending canonical mutation is checkpointed before commit, so a restart
replays the original expected head, action, and attribution instead of inferring success from a
similar-looking later snapshot.

Assigned-agent retirement reuses this durable checkpoint machinery. Graceful stop failure or
uncertainty blocks the assignment; explicit force is required before ending it and authoring the
absorbing retirement fact. Startup repair and response-loss replay retain the exact operation,
request digest, expected project head, action, and runtime observation. Idle retirement needs no
saga row and is validated again in the fact commit transaction. The existing unshipped storage v13
schema and local API v1 were extended in place: there is no migration, compatibility facade, or
version bump.

Explicit open and resource add/replace commands re-observe the exact desired display/canonical
identity before commit. The serialized canonical callback then repeats lifecycle, authority,
expected-head, normalized path identity, and home-qualified claim checks against the complete
post-mutation resource set. Closed projects may retain overlapping desired resources; opening or
mutating an open project may not acquire a conflicting active claim. Remove requires explicit
force while assigned, and replace authors one atomic fact. These commands only change HQ's desired
membership and advisory claims; they never mutate Git or filesystem state.

Close begins with one stable, batched, read-only release assessment. Clean and non-Git resources
may proceed gracefully; dirty or unknown observations require explicit force. A graceful close
authors `ProjectClosingStarted` before asking the exact assigned runtime to stop, so dispatch is
disabled while the assignment and every active claim remain authoritative. Definite runtime
failure or an unresolved stop response leaves that closing state intact. Explicit force may end HQ
authority after a failed or uncertain stop, but the assignment-end and closed facts retain the
typed runtime observation and do not assert that an external process stopped. Final close releases
advisory claims without invoking any mutating resource capability and preserves desired resources,
pending inputs, threads, and history.

Archive has no implicit force. An open archive request uses the same graceful close path and only
authors `ProjectArchived` after the project is closed and unassigned. Archive and unarchive on an
already closed project call neither runtime nor resource capabilities; unarchive remains closed and
claim-free. Every release, runtime, assignment-end, close, archive, and unarchive boundary uses the
existing saga effect and pending-canonical-mutation checkpoints, so response-loss repair needs no
additional storage field or compatibility migration.

Handoff first validates a distinct idle active agent and one historical project thread previously
attributed to that agent. The old runtime is then stopped by exact assignment/session identity. A
failed or unknown graceful stop authors `ProjectAssignmentBlocked`, retaining the old assignment
and claims while disabling dispatch until a human submits a separate forced takeover. Force may
author an assignment end with a failed or uncertain observation, but only revokes HQ authority; it
does not assert external cessation. After the old end commits, handoff reuses the ordinary resource,
configure, runtime-readiness, launch-directory, runnable, and pending-dispatch path. Failure there
compensates to open/unassigned and never resurrects the old epoch.

Retirement uses the same quiescence/block/force policy when the named agent owns the current
assignment, then leaves the project open and claim-preserving. An already idle local active agent
skips runtime control. The final `AgentRetired` fact is installation-private, cites the exact active
claim and selected-session frontier, and is committed only after a serialized global check finds no
assignment for that agent. Retirement is absorbing in the agent reducer while names, sessions,
threads, messages, dispatches, output, and late-output attribution remain history. These workflows
reuse the existing failure, effect, operation, selected-thread, and pending-mutation fields; they add
no storage schema or migration.

Pending project inputs remain separate and ordered by their home acceptance sequence. Each exact
input is submitted through the harness supervisor's existing durable delivery ledger. The workflow
reconciles an uncertain submission before retry and authors `ProjectInputDispatched` only after
that ledger reports definite acceptance. The workflow adds no second provider queue and never
concatenates backlog messages.

Project workflow intake uses one public passive `ProjectCommandRequest` with stable command and
operation identities, an exact digest, account/project/home identities, an optional expected
project head, explicit issue time, and a closed `ProjectCommandAction`. Every existing-project
action requires `Some(head)`; only `ProvisionWorktree` requires `None`, because no previous project
exists. Results are typed accepted, running,
completed, rejected, or reconcilable outcomes with an explicit durable checkpoint. The
`ControlProjects` capability is opaque because its implementation owns project serialization and
bounded recovery; the request, action payloads, provisioning request, and outcome fields are not
hidden behind accessors.

Provisioning reserves the normalized destination before crossing Git, persists a stable Git
operation before create, and always performs exact lookup before a retry. The Git adapter validates
the branch and proves the destination's top-level worktree, common repository, and symbolic branch;
conflicting registrations, branches, files, symlinks, and detached or mismatched worktrees fail
closed. Once created, the workflow identifies the path through the read-only resource capability,
persists the exact healthy resource in the pending canonical creation, and authors one open
`ProjectCreated` fact with no previous-state authority. Definite pre-Git rejection and committed
project ownership release the temporary reservation. Accepted or uncertain Git state retains the
reservation after a later rejection, and no recovery path prunes, resets, deletes, or overwrites
external Git state.

`ProjectCommandRouter` sends local-home requests directly to that workflow. A non-home call authors
only a strict versioned `RemoteProjectCommandRequested` fact and reports `AwaitingHome`. The home
worker scans a bounded deterministic projection, authors an exact receipt before execution, drives
the same saga, and authors a terminal outcome only for definite completion or rejection. Receipt,
saga, and outcome uncertainty remains repairable under stable identities. The application-backed
remote port builds each request, receipt, and outcome plan from one serialized snapshot, checks
digest/body agreement and expected heads, and cites exact request/receipt, project-head,
active-human, and project-home authority facts.

## Mailbox messaging and discovery

Message authoring enters through pure application planners. `MessageAuthoringAuthority` carries the
exact author installation, semantic sender mailbox, audience scope, historical authority edge, and
complete support set. Question, asynchronous-message, reply, cancellation, archive, and restore
requests are passive public-field records. The planners reject cross-installation private routes,
scope changes, non-reversed replies, cancellation by a mailbox other than the question sender,
non-root-derived thread identities, and incomplete state frontiers before a fact reaches an
adapter. The validated `FactPlan` remains opaque because it owns the encoded authoring invariant.

Conversation pages pair each `MessageView` with its normalized `ThreadView`, so clients consume the
reducer's canonical ready-answer and cancellation decisions instead of reconstructing causality.
Page DTOs expose typed sender/recipient, purpose, presentation, reversible-state frontier, receipt
children, root identities, ready-answer state, and cancellation state. Dependency-incomplete
message facts remain separate inert snapshot diagnostics with bounded missing/unusable dependency
sets and explicit truncation; displaying one never grants reply, cancellation, archive, or delivery
authority.

Provider-session discovery joins permanent direct-session bindings with the mailbox's grow-only
`RepositoryContext` history. The resource adapter observes a canonical current directory, Git
common repository, worktree, and symbolic branch through bounded read-only operations. An explicit
session always supplies both provider and provider-scoped session; opaque session text never causes
provider inference.

Agent waits have no overall deadline unless the caller requests one, but each snapshot, page, and
connection attempt retains a fixed bound and reconnect retry budget. Ready delivery is at least
once: the executable writes stdout before authoring the reversible archive completion. Failure in
that window repeats a stable message identity instead of losing content. Direct `get` and human
list operations do not create a completion. The clean unshipped local API v1 and storage v13
contracts were completed in place; no migration, compatibility accessor, or version bump exists.

## Subscription revision race

Subscription preparation has three ordered phases:

1. `ObserveRevisions::register_subscription` creates a pending observer.
2. `QueryDomain::authoritative_snapshot` reads the revision and all projection packages.
3. The caller writes that snapshot acknowledgement, then separately invokes
   `activate_subscription`.

If snapshot loading fails, the service cancels the pending observer before returning the query
error. Preparation never activates delivery. This lets the local-session adapter buffer/coalesce
changes after registration without delivering before its acknowledgement has been written. Active
or pending registration cancellation is idempotent.

## Store adaptation and acceptance

`hq-store::StoreGateway` is configured with an explicit `AuthorityPolicy` and a shared signer
capability. It implements only `QueryDomain` and `CommitFacts`; the node combines it with separately
owned relay, runtime, resource, and observation adapters. The store actor loads revision and all four
projection packages in one serialized request. Health likewise loads revision and its normalized
index in one serialized request, and explicit repair returns the repaired index and observed
revision without allowing another actor request to interleave. Mutation decisions enter the existing atomic local
commit path, and retained result bytes are strictly decoded back into application receipts.

Contracts prove exact replay does not decide twice, changed-digest reuse conflicts before decision,
pure rejection and commit translation, post-commit wake independence, accepted and uncertain
external outcomes, registration/query/activation order, query-failure cancellation, and store
gateway equality. The architecture verifier forbids runtime and adapter concerns in
`hq-application` and permits the dependency only from adapters toward it.
