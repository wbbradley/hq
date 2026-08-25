# Project model

Status: implemented v1 model. This document is normative for project semantics and invariants.

HQ projects coordinate durable conversations, named agents, execution threads, and exclusive
claims on local work areas. The model is advisory at the operating-system boundary: HQ can prevent
cooperating HQ actors from receiving overlapping assignments, but it cannot prove that arbitrary
processes have stopped accessing a directory.

## Goals

- Prevent two HQ-managed agents from being assigned overlapping work areas.
- Let a durable named agent move between distinct lines of work without making the agent own those
  work areas permanently.
- Keep a conversation attached to its line of work even when that work is closed, reopened, or
  handed to another agent.
- Support projects containing several repositories, Git worktrees, ordinary directories, or
  eventually other exclusive resources.
- Treat every UI as a potentially remote, asynchronous client while retaining one clear authority
  for mutations.
- Preserve enough immutable history to explain assignments, dispatch, failures, and forced
  takeovers after the fact.

## Vocabulary

### Project

A project is a durable work area with an immutable UUID and one immutable home HQ installation. It
has mutable display metadata, a desired set of resources, one project mailbox, lifecycle state, and
assignment history. A project can be opened and closed repeatedly, but it is never deleted.

A mutable name is only a display label. Names need not be globally unique. UIs disambiguate them
with the home installation and a shortened project UUID.

A project may have an optional mutable human-authored brief. If omitted, a client may derive an
initial brief from the first project message. A new execution thread receives the brief and relevant
operational metadata, but HQ does not automatically inject the complete mailbox history.

A new project may carry an optional immutable `predecessor_project_id`. This is a one-way lineage
link for deliberate continuation or recovery from a lost home; it does not require the predecessor
to be reachable or mutable. Branching and merging project lineage are deferred.

### Agent

A named agent is a durable mailbox identity local to one HQ installation. It does not own a project
or a worktree. An agent can be currently assigned to at most one project, and a project can be
currently assigned to at most one agent. Historical relationships are retained.

`idle` means exactly `unassigned`. Offline, stopped, or between turns does not make an assigned
agent idle.

### Resource

A resource is an immutable typed locator in the namespace of one project's home installation. V1
implements only `path` resources, while the schema and domain API use the general term `resource`.
Future resource kinds might represent Docker containers, exclusive client connections, or other
coordinated capabilities. Each kind owns its canonicalization, conflict, health-check, and display
semantics.

A resource UUID, kind, home, and canonical locator are immutable. Moving or renaming a path is an
atomic replacement operation: acquire the new resource, update the project's primary resource if
needed, and release the old claim. External unobserved moves leave the old resource degraded until
the human reconciles it.

### Project mailbox

A project mailbox is the durable address and queue for project work. The currently assigned agent
serves it. The mailbox survives closing, archival, agent reassignment, and thread replacement.

The mailbox is an application authorization and routing boundary, not a cryptographic principal.
Nostr wrappers are encrypted to installation root keys. The home daemon decrypts them, stores local
state in plaintext SQLite, and enforces mailbox authorization. V1 assumes honest actors inside that
application boundary.

### Execution thread

Every execution thread has immutable scope at creation:

- `direct(agent_id)` for a personal/control-plane agent thread; or
- `project(project_id, agent_id)` for project execution.

A thread cannot move between projects, agents, or scopes. Existing threads created before projects
exist migrate as direct threads rather than being attached to invented projects.

### Protocol and mailbox messages

The generic word *message* appears at two layers:

- A **protocol message** is any envelope exchanged with a daemon, including commands, command
  results, replicated facts, and mailbox traffic.
- A **mailbox message** is conversational content stored in a human, agent, or project mailbox.

Within that distinction:

- A **project message** is durable conversational content addressed to a project mailbox and
  eligible for runtime dispatch.
- A **direct agent message** is projectless content addressed to an agent's personal mailbox.
- A **project notice** is a daemon-authored diagnostic correlated with a project and shown as
  `HQ · project-name`; it is not automatically dispatched as runtime input.
- A **project command** is a typed control-protocol operation about a project, not conversational
  content.
- A **runtime delivery** is the dispatch of one project message to one selected execution thread.

All newly created project mailbox messages use canonical text schema 2. Human instructions and
supplementary explanations remain in `body` and `details`; presentation and harness correlation
use typed fields. Project code must not recover structure from either text field.

Technical sections disclose diagnostic provenance without becoming project state. Current
HQ-owned namespaces are `hq.project.output_provenance` for agent/assignment/thread attribution,
`hq.project.resource_health` for resource notices, and `hq.project.pending_message` for queued
input notices. They are rendered generically and hidden with the same TUI `i` control as any unknown
namespace. Project authorization, assignment choice, delivery, late-output classification, and
other behavior use typed project records and message purpose/correlation—not technical namespace,
key, label, or value checks.

## Core invariants

1. A project has one immutable UUID and one immutable home installation.
2. All of a project's resources, assigned agents, runtime adapters, and execution threads are
   co-located on that home machine in v1.
3. A project has at most one current agent, and an agent has at most one current project.
4. An agent is idle if and only if it is unassigned.
5. Project resource claims and assignments are durable; they never expire because a process,
   daemon, or machine goes offline.
6. Short-lived process/runtime leases may expire, but their expiry does not release a project claim
   or assignment.
7. An open project holds all of its current resource claims even when it has no assigned agent.
8. A closed project has no agent and holds no resource claims.
9. An archived project is closed and unassigned.
10. A runnable assignment has one selected execution thread scoped to both that project and agent.
11. No client, including the TUI, mutates SQLite directly. The authoritative daemon performs every
    mutation through typed domain operations.
12. Closing, archival, and claim release never delete or modify files, worktrees, branches,
    containers, or other underlying resources.

## Relations and history

The normalized storage model should distinguish desired membership from active claims and retain
historical epochs. Exact table names are implementation details, but the domain contains relations
equivalent to:

| Relation | Cardinality and lifetime |
|---|---|
| project to home | exactly one, immutable |
| project to mailbox | exactly one, immutable |
| project to desired resources | one-to-many, mutable and historically recorded |
| project to primary path | zero or one, mutable; defaults to the first human-selected path |
| open project to active claims | one per desired resource, released on close |
| project to current agent | zero or one |
| agent to current project | zero or one |
| project to assignment epochs | one-to-many historical |
| project/agent pair to threads | one-to-many historical |
| assignment to selected thread | zero while configuring, exactly one when runnable |
| project to predecessor | zero or one, immutable |

The model may use tables conceptually named `resources`, `project_resources`,
`resource_claim_epochs`, `project_assignment_epochs`, and `resource_health`. A generic resource row
does not imply generic conflict logic: v1 dispatches only to the path implementation and rejects
unknown kinds.

## Path resource semantics

Path locators are absolute and qualified by the project home. HQ stores the human-facing spelling
separately from the canonical locator used for conflict checks.

Two path claims held by different open projects conflict when their canonical paths are equal or
one is an ancestor of the other. Overlapping paths within one project are permitted. A descendant
claim may be useful as the primary resource even when a parent already supplies exclusion.

Distinct Git worktree roots can be claimed by different projects even when they share one Git
common directory. A short-lived repository-maintenance mutex may serialize operations such as
`git worktree add`; it does not turn the entire repository into a persistent project resource.

HQ may reserve a path before it exists. It resolves the nearest existing ancestor, appends the
normalized missing suffix, and records the resource as missing. When the path appears, a health
check verifies that its symlink resolution still agrees with the reserved locator. HQ never
silently changes resource identity.

Resource claims coordinate HQ actors but are not a filesystem sandbox. A human may explicitly
remove a resource while an agent is assigned. The UI warns about continuing access, and the daemon
records the action in structured logs and audit history.

## Resource health

Each resource kind may implement a read-only `check` operation. Path checks can report states such
as `healthy`, `missing`, `inaccessible`, `malformed`, or `unknown`, plus a timestamp and structured
details.

Health is observational rather than part of the project lifecycle:

- A missing or inaccessible resource does not automatically close the project, unassign its agent,
  or release its claim.
- Thread bring-up validates the selected working directory independently and may fail.
- A broken non-primary resource can warn without necessarily blocking launch.
- V1 checks at meaningful boundaries: project open, project inspection, resource mutation, and
  thread bring-up. Continuous polling is deferred.

The home daemon emits a durable project notice only when the observed condition changes materially:
one notice on degradation, another when the error changes, and a recovery notice when healthy
again. Repeated identical observations update `last_checked_at` without spamming the conversation.

## Primary path and launch directory

A project with path resources may designate one explicit primary path. It initially defaults to the
first path selected by the human and can be changed without reordering resource membership.

New-thread `cwd` defaults to the primary path. The human may override it with any absolute directory,
including one outside the project's claims; HQ warns but does not forbid that choice. The home
daemon checks the selected directory at thread bring-up and reports missing, non-directory, and
access failures explicitly. It never silently substitutes another directory. Each thread records
the actual launch directory, and changing the project default affects only future threads.

Resuming a thread whose recorded directory is missing or no longer claimed requires an explicit
human decision rather than silent relocation.

## Project lifecycle

The durable lifecycle is reversible `open <-> closed`, with `closing` and `preparing` as operational
transition states. `archived` is a presentation flag constrained by `archived => closed`.

### Create and open

Creating a project chooses its immutable home, UUID, optional predecessor, name, optional brief,
desired resources, and primary path. Opening atomically acquires all desired resource claims on the
home daemon. An open project may remain unassigned indefinitely.

Adding a resource to an open project is allowed through a daemon transaction that validates and
acquires its claim. Removing a resource is an explicit human-authorized operation and is permitted
while assigned, with warnings and audit records.

### Close

Normal close enters `closing`, stops accepting new dispatch, retains claims, and asks the local
runtime adapter to quiesce. Once the active turn and worker have stopped, one authoritative mutation
unassigns the agent, releases all claims, and marks the project closed. Historical desired resource
definitions, messages, assignments, and threads remain.

Force close is human-authorized and may proceed when the external runtime cannot be proven stopped.
It revokes HQ dispatch authority and releases advisory claims, but runtime observation may remain
`still-running` or `unknown`. HQ must never equate closed project state with proof that an arbitrary
actor stopped accessing files.

### Reopen

Reopening attempts to reacquire every desired resource claim atomically. A conflict leaves the
project closed with no partial claims. Reopening does not require immediate agent assignment.

### Archive

Archiving an open project first performs the normal graceful close workflow. Unarchiving makes the
project visible but closed; it does not reacquire claims or assign an agent. Archived projects
remain searchable, readable, and permanent.

New activity addressed to an archived project is accepted and surfaced in the human inbox without
automatically unarchiving or reopening it.

## Assignment and activation

Assignment is independent of opening except for the invariant that a closed project cannot retain
an agent. Assigning requires an open project and an idle agent co-located on its home.

An assignment begins in `configuring`. It becomes `runnable` only after HQ has selected or created
an execution thread scoped to that project and agent. Assigning a different agent always requires a
new thread. Reassigning an earlier agent may explicitly resume one of that pair's historical threads
or start fresh.

Project messages remain queued until the assignment is runnable. Once runnable, pending messages
dispatch automatically in the project's authoritative order, each as a distinct runtime input.
HQ does not concatenate the backlog into a synthetic prompt.

Opening, assignment, thread selection, and initial send may be exposed as one compound user action,
but external runtime startup cannot share the SQLite transaction. It is therefore an activation
saga with compensation:

- If a previously closed project fails during thread bring-up, release newly acquired claims and
  return it to closed/unassigned.
- If an already open project fails during thread bring-up, keep it open but release the new
  assignment.
- Preserve the human message as pending and retain detailed failure diagnostics.
- Never leave the project or agent indefinitely stranded in `configuring`.

Normal reassignment requires confirmed runtime quiescence. If the old actor is unavailable or
uncooperative, the project enters a blocked handoff state. A human may explicitly force takeover,
which revokes the old assignment's HQ authority and assigns another idle agent after a prominent
warning that the old actor might still access the resources.

Retiring an assigned agent first quiesces or force-stops its runtime and unassigns it. The project
stays open and retains its claims, pending messages, and historical attribution while awaiting a
replacement. Threads attributed to a retired agent cannot be resumed.

## Project conversations and dispatch

Human work is addressed to a project, not directly to its currently assigned agent. Direct agent
messaging remains a separate control-plane facility.

Project messages are immutably bound to the project and their causal conversation, not necessarily
to an execution thread. This matters because a message can be created while the project is closed,
unassigned, or between threads.

The home daemon sequences every valid and causally usable project input. It emits a signed
acceptance/sequence fact so dispatch never depends on relay receipt order or client timestamps. A
separate home-signed dispatch record binds an input message to the assignment epoch, agent, and
thread that actually received it. Each input is dispatched at most once, using the bridge's durable
idempotency and reconciliation machinery.

Agent output retains immutable agent, thread, and assignment-epoch provenance. The project mailbox
remains the conversation address and reply target. UIs present dual attribution such as
`bob · project-name` rather than pretending the project itself authored the output.

The output message preserves caller-supplied human details, typed presentation and correlation, and
ordered technical sections, then appends one ordered `hq.project.output_provenance` section. The
project's assignment and thread tables remain authoritative for behavior; the section is
display/diagnostic provenance. Stable project-output retries compare the complete typed message and
provenance. An identical retry is idempotent, while a changed presentation, correlation, technical
field, or human payload under the same deterministic ID fails as a collision. Late output appends
diagnostic old/current attribution without granting the inactive assignment authority.

A reply in an old conversation remains addressed to the project. If the project is closed, HQ
persists it as pending activation and does not silently reopen, reassign, or resume a stale thread.
The human may reopen with the prior agent and thread, choose a fresh thread, assign another idle
agent, or create a successor project.

The currently assigned agent is authorized to query the project's complete mailbox history, but
only pending messages are automatically delivered into its selected thread. Current runtimes need
not expose an HQ history tool to the model; model-visible project tooling is a future capability.

If an old runtime emits output after unassignment or force takeover, HQ retains it and marks it
`late from inactive assignment`. It cannot mutate project state or masquerade as current output.
The daemon logs the old and current assignment epochs, agent, thread, runtime owner token, message
ID, observed runtime state, and whether the transition was forced.

Project-message cancellation is not supported in v1.

## Direct agent control plane

An agent may have a projectless personal mailbox and direct thread on a separate conceptual plane.
This can eventually support inspecting its assignments, coordinating with agents, and messaging
project or personal mailboxes. Direct messaging does not implicitly open a project, acquire a
resource, create a project thread, or authorize project execution.

The detailed capabilities, scheduling, and model-visible tools for this control plane are deferred.
For project initiation, the UI is project-first; agent-first composition means direct messaging.

## Compose flow

A new project-message composer begins with project selection:

- **Runnable project:** compose immediately.
- **Open, unassigned project:** choose an idle local agent and select or create a compatible thread.
- **Closed project:** preview resource conflicts, then reopen, assign, and select a thread.
- **New project:** choose the home, desired resources, primary path, optional brief, agent, and
  execution thread.

The UI should suggest a project's most recently assigned agent when that agent is idle, reflecting
the human's temporal affinity between an agent and a line of work without making the affinity an
ownership rule. A compound activation failure leaves the composed message pending and restores the
prior stable project/assignment state.

Git worktree creation is one option inside new-project resource selection. Its modal can choose the
repository, merge base, worktree destination, branch name, and primary path. Provisioning is an
explicit daemon workflow with a temporary destination reservation. Closing or archiving never
removes the created worktree or branch.

The CLI exposes the same workflow as `hq project worktree`; both surfaces use one stable operation
ID. A retry resumes after reservation, Git creation, or project creation rather than creating a
second worktree or project.

## Authority, consistency, and transport

### Home authority

The project home daemon is the sole mutation authority and dispatch sequencer. Paths and locks are
meaningful only in its machine/resource namespace. Project re-homing and remote-agent assignment
are not v1 features.

If the home installation identity is permanently lost, replicas retain a read-only orphan. Pending
commands eventually report the home unreachable; no replica assumes authority. Restoring the
original identity restores authority, or the human can create a successor project elsewhere.

### Human authorization

Only active human-account devices may authorize project lifecycle, resource, assignment, takeover,
or archival mutations in v1. Agents may inspect authorized state and send messages or suggestions,
but cannot directly mutate those safety boundaries. The home daemon remains the final validator.

### Linear history and compare-and-swap

Each project has one linear, home-issued authoritative event history. Every mutation command except
creation includes the content-addressed `expected_head_event_id`. The home compares it with the
current head and rejects stale commands with the current revision and state summary. The event ID
already commits to exact canonical bytes, so a second content hash is unnecessary.

One client may transmit at most one unresolved state-changing command per project. Multi-step UI
actions use one compound domain command. Later local intents remain drafts until the earlier result
supplies a new head. Commands from different devices can race; one commits and the others receive
stale-head conflicts.

### Remote-by-default clients

The TUI, CLI, mobile controller, and future clients are conceptually remote and asynchronous even
when they share a machine with the authority. A local socket is only a low-latency first hop. Clients
never report authoritative success merely because their local daemon durably queued an intent.

Every command has observable stages:

1. accepted durably by the client's local daemon;
2. queued or relayed;
3. received by the project home;
4. authoritatively committed or rejected; and
5. runtime side effects converged, failed, or remained unknown.

The current command-result envelope reports `received` and terminal `committed` or `rejected`
stages. Runtime commands are not reported committed until their runtime saga converges; a rejection
diagnostic distinguishes a definite failure from an external runtime whose state remains unknown.

Commands may be queued while the home is offline. The UI shows pending state until it receives a
signed result. Expected-head comparison prevents a delayed command from silently applying to state
the human did not witness.

### Transport roles

| Channel | Responsibility and guarantees |
|---|---|
| Local domain RPC | Typed requests from a local client to its daemon; retry uses stable IDs. |
| Remote control protocol | Encrypted, authenticated, durable command and result envelopes routed over Nostr in v1. |
| Replicated project events | Home-signed authoritative facts distributed to all active human-account devices. |
| Mailbox traffic | Encrypted conversational project, human, and agent content. |
| Local runtime adapter | Best-effort control and observation of an external actor such as `codex app-server`. |

Remote commands reuse HQ's Nostr encryption, authentication, durable outbox, retry, and
deduplication machinery, but have a separate control schema and inbox/outbox from conversational
mailboxes. A future synchronous daemon transport may optimize latency while preserving identical
domain semantics.

Every active human-account device receives the encrypted project-state projection needed for remote
control: project identity, home, lifecycle, resources, assignment, selected thread, health, and
pending command status. Per-project device ACLs are deferred.

## Transaction and runtime boundaries

The home daemon serializes project mutations. Resource conflict validation, active claim changes,
assignment changes, event append, projections, mutation receipts, and revision increments should
commit in one SQLite transaction whenever they are database-only.

Filesystem operations, Git operations, network delivery, and runtime start/stop acknowledgements
cannot participate in that transaction. Workflows crossing those boundaries use explicit
transition states, stable operation IDs, idempotent retries, and compensating actions. SQLite is the
authority for HQ coordination state; it is not evidence that an external side effect occurred.

The TUI and other clients must use versioned daemon RPC/domain commands. Reaching into SQLite for
project creation, locking, assignment, or repair is unsupported.

## Diagnostics and audit

User-facing failures identify the conflicting object and a safe remediation. Structured logs and
durable audit history carry enough detail to reconstruct decisions, especially for resource
conflicts, stale commands, activation compensation, force close, force takeover, and late runtime
output.

A path-lock conflict should record at least:

- operation, stable request ID, requesting human device, project, and proposed agent;
- requested, display, and canonical paths;
- conflicting project and claimed path;
- overlap type: equal, ancestor, or descendant;
- project lifecycle, assignment, and runtime observations;
- expected and current project heads; and
- timestamp, home installation, and final outcome.

Logs must avoid environment contents, credentials, message bodies, and other sensitive runtime data
outside the existing protected diagnostic boundary.

## Deferred work and non-goals

- Project re-homing or execution across several machines.
- Assigning a remote agent to a project.
- Synchronous remote RPC as anything other than a transport optimization.
- Per-project device ACLs or per-agent end-to-end encryption keys.
- Generic resource kinds beyond the monomorphic path implementation.
- Enforcement against non-HQ processes or a mandatory filesystem sandbox.
- Continuous resource-health polling.
- Detailed direct-agent control-plane tools and scheduling.
- Automatic full-history injection or generated handoff summaries.
- Project-message cancellation.
- Project deletion, destructive worktree cleanup, or automatic branch deletion.
- Rich project-lineage branching or merging semantics.
