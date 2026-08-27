# Completed

## 2026-08-26 — Durable installation-local TUI drafts

Added unsigned, installation-local SQLite drafts with optimistic versions and wire-7 RPC/client operations. The TUI now restores drafts before its first render, coalesces serialized autosaves over a documented 250 ms abrupt-loss window, requires successful persistence before stow or graceful quit, explicitly deletes canceled or emptied drafts, and retains bodies on failures and stale targets for reselection. Atomic submission uses the stable draft UUID as message identity, commits normal replies or project inputs with draft consumption in one transaction, preserves project activation intent, restores target wakes on RPC replay, and prevents duplicate messages after lost responses. Store, RPC, client, TUI, restart, stale-target, debounce-order, rollback, project-input, replay, full-suite, vet, build, and focused race tests pass.

### Original plan entry

## Durable installation-local TUI drafts

- Add unsigned `tui_drafts` storage and domain/wire-7 DTOs for `TUIDraft`, `ListTUIDrafts`,
  `PutTUIDraft`, `DeleteTUIDraft`, and `SubmitTUIDraft`. Persist a stable UUID, optimistic
  version, body, reply target/conversation or new-message recipient, address/label, repository
  context, domain-level project activation intent, and timestamps. Drafts never become
  canonical/Nostr or replicated state.
- Load drafts before first render. Preserve current draft rows and resume behavior. Autosave active
  edits after a 250 ms coalescing window, serialize saves with optimistic versions, and force a
  successful save before stowing or graceful quit; failed saves keep the editor open with an error.
- Use the stable draft UUID as message/idempotency identity. `SubmitTUIDraft` durably commits the
  message or pending project input and consumes the draft as one recoverable mutation. Lost replies
  retry without duplication; failed sends retain the draft; emptying explicitly deletes it.
- Keep stale-target drafts visible and require recipient/project reselection rather than dropping
  their bodies. Test TUI/daemon restart, invalidation reload, stale targets, ordered debounce,
  failure retention, successful consumption, and the documented abrupt-loss debounce window.

## 2026-08-24 — Durable agent and project work reconciliation

Added a durable pending-work projection for direct named-agent inboxes and runnable project assignments, including persisted selected threads and launch directories. The supervisor now runs an observer-safe initial scan plus coalesced invalidation and periodic repair scans, reusing existing worker/waking guards and automatic resume validation while treating explicit RPC wake environments as optional latency hints. Node startup installs the observer before reconciliation begins. Store, supervisor, restart, exclusion, invalidation, duplicate-trigger, full-suite, vet, and race tests cover convergence without a second message.

### Original plan entry

## Durable agent and project work reconciliation

Replace best-effort RPC wake calls with a supervisor reconciliation loop driven by durable pending work. This depends on canonical project-input acceptance so the supervisor has one reliable source of truth.

Scope:

- Add store queries that describe runnable pending work for direct named agents and project assignments.
- Reconcile pending work after the supervisor and store observer are installed during daemon startup.
- Reconcile on relevant message, project, assignment, and delivery invalidations.
- Start or resume offline workers when durable selected-thread and repository state make them runnable.
- Treat the sending client's environment as an optional launch hint, not a correctness dependency.
- Coalesce concurrent invalidations and make reconciliation idempotent with existing worker and waking guards.
- Retain explicit wake calls only as latency optimizations, or remove them if reconciliation is immediate enough.
- Handle pending work committed by remote ingestion, startup repair or rebuild, direct store callers, and a process crash after commit but before RPC wake.

Primary areas:

- `internal/codexsupervisor/supervisor.go`
- `internal/node/node.go`
- `internal/domain/codex_runtime.go`
- `internal/domainrpc/server.go`
- Project and named-agent store query implementations
- `internal/codexsupervisor/supervisor_test.go`
- Node integration tests

Risks:

- Reconciliation must not launch archived, closed, unassigned, retired, or otherwise non-runnable targets.
- Startup ordering must avoid losing invalidations between the initial scan and observer registration.
- Repeated scans must not create duplicate workers or duplicate dispatches.

Acceptance criteria:

- Restarting the daemon alone eventually delivers already-accepted work to a runnable offline project assignment.
- Remote messages wake eligible offline project and direct-agent workers without a local Create or Reply RPC.
- A fault injected after commit but before explicit wake converges after restart.
- Running workers are not duplicated, and non-runnable projects or agents remain offline.
- Existing selected thread and working-directory state are honored after restart.

Implementation plan:

- Add a focused durable pending-work query that returns one launchable target per direct named-agent mailbox or runnable project assignment, including the selected Codex thread and persisted launch directory, only when incomplete delivery exists.
- Give the supervisor an idempotent reconciliation loop with a buffered trigger and periodic repair scan. Start it explicitly after node change-observer installation, trigger it from message/project/agent invalidations, and run an initial scan so startup cannot miss already committed work.
- Feed durable targets through the existing automatic wake paths, preserving their worker and `waking` guards, last-known-good launch configuration, persisted thread selection, project binding validation, and local daemon environment fallback.
- Keep explicit RPC wake calls as optional low-latency/environment hints; correctness comes from the durable scan, including relay ingress, direct store calls, rebuild, and commit-before-wake crashes.
- Add store query tests for direct/project eligibility and exclusion states, supervisor startup/invalidations tests for direct and project work, and duplicate-trigger tests proving one worker launch and eventual dispatch.

Risks and decisions:

- Runtime ownership leases can briefly outlive a crashed daemon. Periodic reconciliation must retry pending targets after lease expiry rather than treating the initial ownership conflict as terminal.
- The observer is installed after supervisor construction so subscribers can safely receive synchronous store publications; the node must install it before starting the initial reconciliation scan.
- A durable scan supplies the daemon environment only when no in-memory last-good request or explicit wake hint exists; environment remains transient and is never stored in the pending-work DTO.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.


## 2026-08-24 — Mandatory project runtime contracts

Added a daemon-only composite project runtime store contract spanning core operations, project mutations, delivery, output, commands, workflows, and durable pending work. The supervisor now requires that contract and no longer discovers mandatory capabilities at runtime. Codex bridge project delivery and output dependencies are explicit and narrowly scoped, while direct-mode and RPC client interfaces remain small. Added compile-time assertions for SQLite, the RPC client, and supervisor runtime roles, and removed mandatory-capability fallback branches and type assertions.

### Original plan entry

## Mandatory project runtime contracts

Make core project-runtime dependencies compile-time requirements while retaining small optional interfaces only at genuine extension boundaries. Do this after the domain refactors so the final required capabilities are known.

Scope:

- Define a composite daemon-side project runtime store contract covering the project, workflow, delivery, output, command, and pending-work capabilities required by the supervisor and Codex bridge.
- Change node and supervisor constructors to require the appropriate contract instead of accepting generic `domain.Operations` and discovering mandatory capabilities through type assertions.
- Give bridge components focused required interfaces where project mode cannot function without them.
- Keep client-facing and test-double interfaces narrower; do not force local SQLite-only delivery internals onto the RPC client.
- Add compile-time interface assertions for SQLite, the RPC client, supervisor, and other concrete implementations against their intended contracts.
- Replace "capability unavailable" runtime branches for mandatory daemon wiring with construction-time failures.

Primary areas:

- `internal/domain/store.go`
- `internal/domain/projects.go`
- `internal/codexsupervisor/supervisor.go`
- `internal/codexbridge`
- `internal/node/node.go`
- `internal/hqclient`

Risks:

- One oversized interface would make unit tests cumbersome; split contracts by consumer and compose them only at the node boundary.
- Truly optional runtime controllers must remain optional where degraded operation is intentional.

Acceptance criteria:

- The daemon cannot compile or start with a store missing mandatory project delivery or recovery capabilities.
- SQLite and each RPC or client implementation have explicit compile-time assertions for the contracts they own.
- Mandatory project execution paths contain no runtime type assertion whose failure indicates an internal wiring error.
- Unit-test fakes remain focused and readable.

Implementation plan:

- Define a daemon-only `ProjectRuntimeStore` in `internal/domain` by composing the existing general, project mutation, workflow, delivery, output, command, and pending-work interfaces; keep the public `Store` and RPC-client contract unchanged.
- Require `ProjectRuntimeStore` in the supervisor constructor, collapse its optionally discovered project/workflow/pending fields into the required store, and remove mandatory-capability nil checks and type assertions from activation, close, handoff, retirement, worktree, remote-command, recovery, and reconciliation paths.
- Split Codex bridge wiring into a required named-mailbox store plus an explicit focused project bridge store. Pass project delivery/output capabilities only for project workers, and remove project-mode assertions from dispatch and output publication.
- Add compile-time assertions for SQLite's daemon contract, the RPC client's public store/runtime/provisioning contracts, and the supervisor's runtime/controller contracts; let compile failures expose incomplete test doubles at their actual consumer boundaries.
- Run focused bridge/supervisor/node tests, then repository-wide vet, tests, and race-enabled project-runtime tests.

Risks and decisions:

- The daemon contract is intentionally broad only at the supervisor/node composition root; bridge subcomponents continue to accept narrow interfaces so direct-mode fakes do not acquire project-only methods.
- Project bridge dependencies remain explicit option fields because direct named-agent bridges legitimately do not need project delivery or output capabilities.
- RPC clients must not implement daemon-local delivery, workflow, or pending-work internals; their existing public store assertion remains separate from the new daemon-only contract.

## 2026-08-24 — Project ingress and delivery conformance suite

Added a reusable project conformance fixture and behavioral matrices for local create/reply, canonical append/replay/rebuild/startup repair, all typed message purposes, human/agent/home/replica destinations, and runnable/unassigned/closing/closed/archived project states. Added remote reorder/replay/restart convergence coverage, the previously missed project Reply followed by an offline daemon restart and automatic worker dispatch, and an SQLite-backed RPC reply integration test that proves semantic acceptance, assignment-bound claimability, and replay idempotency. The full normal, vet, repeated-focus, and race-enabled repository suites pass.

### Original plan entry

## Project ingress and delivery conformance suite

Build a reusable matrix and invariant suite that exercises the complete project-message lifecycle across every ingress and runtime state. Begin fixtures while implementing the preceding capabilities, then make the full suite the final integration gate.

Coverage matrix:

- Ingress: local Create, local Reply, remote canonical append, replayed mutation, canonical rebuild, and startup recovery.
- Destination: human mailbox, direct named agent, home project, and replica project.
- Project state: open and runnable, open and unassigned, closing, closed, and archived.
- Worker state: running, offline, daemon restarted, and worker launch already in progress.
- Message purpose: conversational input, structured protocol answer, project output, and notice.

Assertions:

- Canonical message identity, threading, reply relationship, and original-message archive behavior.
- Exactly one project acceptance for each eligible input and none for ineligible purposes or destinations.
- Deterministic acceptance sequence and project-head progression.
- Durable wake or reconciliation and eventual dispatch to the selected thread.
- No duplicate worker, claim, dispatch, or protocol delivery.
- Correct mailbox kind, label, device attribution, panel badge, and correlated presentation source.

Fault and invariant tests:

- Crash after canonical commit but before explicit wake.
- Restart after acceptance but before dispatch.
- Duplicate and reordered remote canonical delivery.
- Unknown project event and command operations.
- Malformed typed payloads.
- Rebuild from canonical history with empty derived project tables.
- Every acceptance references a projected canonical message.
- Every eligible home-project input has exactly one acceptance.
- Unsupported replica events do not advance the replica head.
- Every supported event and command is registered in all required projections and handlers.
- Every mailbox kind has a typed display mapping.

Primary areas:

- `internal/store/projects_test.go`
- `internal/store/sqlite_test.go`
- `internal/codexsupervisor/supervisor_test.go`
- `internal/codexbridge/*_test.go`
- `internal/domainrpc/server_test.go`
- `internal/node` integration tests
- `internal/tui/tui_test.go`

Acceptance criteria:

- The matrix includes the previously missed Reply x home project x offline or restarted worker combinations.
- RPC tests assert semantic acceptance and dispatch, not only that a mocked method was invoked.
- Fault tests demonstrate eventual convergence without a second user message or manual resume.
- The full suite passes under normal and race-enabled test runs.

Implementation plan:

- Add a reusable store conformance fixture that can inject project-addressed messages through local Create, local Reply, generic canonical append, duplicate replay, rebuild, and startup reconciliation, then assert the shared invariants: one projected message, one acceptance, a canonical message reference, deterministic sequence, and stable project head.
- Add table-driven destination, message-purpose, and project-state coverage. Verify only eligible human conversation addressed to a home project is accepted, every lifecycle preserves acceptance, and only an open runnable assignment is dispatchable.
- Add duplicate/reordered replica-history coverage proving deterministic convergence and preservation of the last valid head, complementing the existing unknown/malformed reducer tests and typed event/command completeness tests.
- Add the specific Reply → home project → daemon restart/offline worker regression test, asserting startup reconciliation launches one worker and dispatches the reply to the assignment's selected thread without a second wake message.
- Add an RPC integration test backed by SQLite that performs a real project reply mutation and asserts canonical acceptance plus assignment-bound claimability, rather than stopping at mocked method invocation.
- Treat existing bridge delivery crash-window tests, supervisor recovery tests, TUI mailbox/presentation exhaustiveness tests, and typed registry completeness tests as matrix rows; run full normal, vet, and race-enabled suites as the final integration gate.

Risks and decisions:

- The coverage matrix is compositional rather than a wasteful Cartesian product: each axis is behaviorally exercised, while high-risk intersections (Reply/home/restart and remote/replay/rebuild) receive dedicated end-to-end cases.
- Canonical replay and startup fixtures deliberately enter below convenience methods so they exercise the same repair boundaries used after crashes and upgrades.
- Worker tests use the scripted app-server protocol already used by supervisor tests, keeping the suite deterministic and network-independent.

## 2026-08-24 — Canonical project-input acceptance invariant

Moved project-input acceptance into the canonical ingest boundary, where one source-agnostic reconciler sequences every eligible human conversation addressed to an authoritative project. Local create/reply, generic appends, remote append, relay receive, startup, and rebuild now converge through the same transaction; the specialized project-message writer and version-specific reply repair were removed. Structured protocol answers remain excluded by typed purpose, closed-project notices stay atomic with unique acceptances, project invalidations include remotely accepted inputs, and recovery/replay tests prove exactly-once behavior.

### Original plan entry

## Canonical project-input acceptance invariant

Centralize project input acceptance as a canonical commit invariant instead of attaching it separately to `Create`, `Reply`, remote append, and repair code paths. This requires typed message purpose so structured protocol answers can be excluded deliberately.

Scope:

- Introduce one transactional reconciliation function invoked after canonical projection whenever messages enter or are replayed.
- Apply it uniformly to local create, local reply, remote `AppendCanonical`, mutation replay, database rebuild, and startup recovery.
- Guarantee that every eligible conversational human input to a home project mailbox has exactly one `project.message.accepted` event and acceptance row.
- Preserve deterministic per-project sequence ordering and correct project-head advancement.
- Keep closed and archived pending notices as part of the same invariant.
- Remove entry-point-specific project detection and the version-specific `repairLocalProjectReplies` path once general reconciliation covers it.
- Ensure retries, duplicate canonical events, and concurrent ingress remain idempotent.
- Make canonical rebuild reconstruct authoritative project acceptance state rather than passing because local project tables were retained.

Primary areas:

- `internal/store/sqlite.go`
- `internal/store/project_inbound.go`
- `internal/store/project_delivery.go`
- `internal/store/projects.go`
- SQLite schema and migrations
- `internal/store/projects_test.go`
- `internal/store/sqlite_test.go`

Risks:

- Acceptance creation itself appends a canonical project event; reconciliation must avoid recursion and double sequencing.
- Project-head compare-and-swap and multiple accepted messages must remain atomic.

Acceptance criteria:

- No eligible projected project input can commit without exactly one matching acceptance.
- No acceptance can exist without its referenced canonical message.
- Replaying or re-appending the same canonical data creates no additional acceptance or sequence.
- Create, Reply, remote append, recovery, and rebuild produce equivalent project state.
- The specialized reply repair is no longer required.

Implementation plan:

- Replace source-specific acceptance helpers with one `reconcileProjectInputsTx` pass over all projected human messages addressed to authoritative local project mailboxes, filtered exclusively by typed conversational/project-input purposes.
- Run reconciliation after canonical projection in the common local append transaction, remote `AppendCanonical`, and explicit rebuild/startup recovery. Let the reconciler sign one acceptance at a time, rebuild authoritative projection, and continue in deterministic project/message order until no eligible gap remains.
- Refactor `Create` and `Reply` to normalize local project conversation purpose before signing but use the generic canonical append boundary; remove `createProjectMessage`, inbound-only filtering, and startup `repairLocalProjectReplies`.
- Keep closed/archive pending notices within reconciliation and make notice generation idempotent by tying it to the unique acceptance event.
- Add invariant checks and tests covering local create, local reply, remote append, rebuild/recovery, duplicate replay, structured protocol exclusion, closed-project notices, and deterministic sequencing.

Risks and decisions:

- Reconciliation appends canonical project events while already inside a canonical-ingest transaction. Guard the common append helper against recursive reconciliation and always recompute pending inputs from the newly rebuilt projection.
- Rebuild may need to append missing acceptance events, so startup rebuild must use the local signer and commit both canonical additions and the final projection atomically.
- Existing acceptance events remain authoritative; reconciliation fills only missing eligible messages and never resequences or replaces established history.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-24 — Authoritative reducer-driven project projection

Made canonical project events the sole authority for local project projections during mutation and rebuild. The shared reducer now retains current and historical resources, every claim and assignment epoch, execution threads, acceptances, and dispatches; new runnable events carry complete thread facts, while legacy resource and thread details are bridged only from existing projections with explicit diagnostics when unavailable. Rebuild now recreates normalized project tables, preserves valid operational saga state, stops safely on forks, updates project mailbox labels, and local mutation, acceptance, and dispatch paths no longer duplicate projection SQL. Added schema migration and clean-rebuild, lifecycle, legacy-compatibility, fork-safety, full-suite, vet, and race coverage.

### Original plan entry

## Authoritative reducer-driven project projection

Make the typed reducer the source of authoritative SQLite project state during live mutation, replay, and clean rebuild.

Scope:

- Extend reducer output to retain project resources, claim epochs, assignment history, execution threads, message acceptances, and dispatch records needed by normalized SQLite tables.
- Include sufficient execution-thread facts in new assignment events, with deterministic compatibility handling for existing histories.
- Rebuild authoritative home projects and their normalized child tables from canonical project history rather than retaining mutable tables.
- Remove local mutation SQL that duplicates reducer application; after canonical ingest, read the reducer-built projection.
- Preserve operational leases and saga records only where they are intentionally noncanonical, and reconcile them against rebuilt authority.
- Preserve fork safety by stopping at the last unambiguous authoritative child.

Acceptance criteria:

- Local mutation, canonical replay, replica projection, and clean rebuild apply the same reducer semantics.
- A clean rebuild recreates projects, assignments, resources, acceptances, dispatch records, and lifecycle state from canonical history.
- Existing canonical histories remain openable; missing legacy thread detail is handled explicitly and never fabricated silently.
- Direct SQL mutation can no longer diverge from canonical project history.

Implementation plan:

- Extend `internal/projectstate` snapshots with applied-event metadata, resource acquisition facts, assignment epochs, project-thread facts, acceptances, and dispatches. Extend `AssignmentRunnable` with an optional typed thread snapshot for new canonical events.
- Add a store-side authoritative history collector that groups projected home-issued `project.event` records, follows only the unique child chain from the creation root, decodes each typed event, and retains the last valid head plus diagnostics.
- In `internal/store/sqlite.go`, capture legacy thread rows before projection, clear rebuildable project tables in dependency order, and reinsert projects, project events, current resources and claims, assignment history, threads, acceptances, and dispatch records from reducer snapshots. Reconcile noncanonical attempts and provenance against rebuilt authority.
- Use existing legacy thread rows only when an old runnable event lacks its now-required thread snapshot. A clean history with missing detail must stop with an actionable diagnostic rather than fabricate a launch directory or external thread.
- Refactor `CreateProject`, every mutation closure in `internal/store/projects.go`, project acceptance paths, and dispatch recording so canonical ingest/rebuild performs projection writes. Remove duplicate inserts and updates after event append.
- Add tests that empty all rebuildable project tables and recreate lifecycle, resources, assignment history, thread details, acceptances, and dispatches solely from canonical history; compare local and replica visible state; and verify legacy thread compatibility and fork/invalid-event retention.

Risks and decisions:

- Operational leases, activation/runtime/worktree sagas, and output provenance are not canonical project state. Preserve them when their rebuilt references remain valid and delete only rows whose authority disappeared.
- Rebuilding on every canonical ingest makes event application atomic with its canonical fact, but mutation code must not retain any post-ingest projection write or it will duplicate reducer output.
- Historical resource membership and claim rows are rebuilt only to the fidelity needed by current state and referenced durable facts; canonical project events remain the complete audit history.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-24 — Typed project event codec and replica reducer

Introduced a closed 18-operation project event vocabulary, typed payloads, audit-envelope decoding, and a pure reducer for lifecycle, resources, assignments, acceptances, and dispatches. Converted every local event emitter to typed data and made replica projection stop at the last valid head on forks, unknown operations, malformed data, or invalid transitions, with audit diagnostics and complete reducer/codec coverage.

### Original plan entry

## Typed project event codec and replica reducer

Create the exhaustive typed project-event vocabulary and pure reducer boundary, then make replica projection use it.

Scope:

- Define typed project-event operation constants and typed payload data for every currently emitted project event.
- Decode the existing canonical audit envelope into typed events while preserving wire compatibility.
- Validate operation-specific payloads and state transitions before applying them.
- Replace `applyReplicaProjectEvent`'s permissive string switch with the shared pure reducer.
- Stop at unknown or malformed events without advancing the replica head; surface a useful diagnostic.
- Change local project event emission helpers to accept only registered typed operations.
- Add completeness tests proving every emitted operation is registered and every registered operation has reducer coverage.

Acceptance criteria:

- Local event emitters cannot pass arbitrary operation strings.
- Replica state is produced exclusively by the typed reducer.
- Unknown or malformed events preserve the last valid state and head.
- Every supported lifecycle, resource, assignment, acceptance, and dispatch operation has a typed codec and tests.

## 2026-08-23 — Typed message purpose and mailbox addressing

Added canonical typed message purposes with deterministic legacy defaults, SQLite migration and rebuild support, RPC/client round-tripping, and purpose-aware project and protocol reply handling. Added typed sender and recipient addresses for human, agent, project, and remote presentation; moved TUI rendering to those values; kept panel kind and attribution correlated; and covered the behavior across model, event, store, bridge, node, and TUI tests.

### Original plan entry

## Typed message purpose and mailbox addressing

Make semantic distinctions that currently have to be inferred from generic `model.Message` fields explicit.

Scope:

- Introduce a typed message-purpose model that distinguishes ordinary conversation, conversational project input, structured protocol questions and answers, project output, and system notices where applicable.
- Carry purpose through canonical event payloads, SQLite projections, RPC requests, and client calls. Preserve compatibility with existing canonical events by defining deterministic defaults for records without an explicit purpose.
- Replace string-parsed sender and recipient presentation with typed mailbox addresses containing mailbox kind, mailbox ID, installation ID, and display label.
- Make all `MailboxKind` handling exhaustive. Project mailboxes must never fall through to agent or remote formatting.
- Update the Codex question/reply path so a structured answer is claimed only as a protocol response and is not also sequenced as conversational project work.
- Review `CreatePeerMessage` and other alternate envelope builders; remove them if obsolete or route them through the same typed construction boundary.
- Update TUI grouping to return one correlated presentation value containing the semantic kind and source message, rather than calculating the badge and sender from potentially different messages.

Primary areas:

- `internal/model/message.go`
- `internal/event`
- `internal/domain/store.go`
- `internal/store/sqlite.go`
- `internal/codexbridge/questions.go`
- `internal/codexbridge/replies.go`
- `internal/codexbridge/output.go`
- `internal/hqclient`
- `internal/domainrpc`
- `internal/tui/tui.go`

Risks:

- Old events do not contain the new discriminator, so fallback behavior must be stable across rebuilds and replicas.
- Structured answers already stored as project acceptances may need compatibility handling without being redelivered.

Acceptance criteria:

- Message purpose and mailbox identity are never inferred by parsing display labels or free-form `Details`.
- Conversational project replies are eligible for project sequencing; structured protocol replies are consumed only by their registered waiter.
- Human, agent, project, and remote addresses render correctly in inbox rows and panels.
- A panel badge, title, and sender are derived from the same source message.
- Existing databases and canonical histories open and rebuild without changing established message behavior.

## 2026-08-22 — Daemon-supervised durable Codex agents

Implemented daemon-owned named Codex workers with exact session history and routing, transient caller environment and working-directory inheritance, idempotent local runtime RPC, shared concurrent delivery checkpoints, detached CLI launch acknowledgements, asynchronous TUI agent/session controls, durable-name developer instructions, schema/reducer rebuild support, and control-plane/data-plane documentation. Added store, reducer, RPC/client, supervisor, bridge, CLI, TUI, node lifecycle, environment privacy, outbox isolation, retry, concurrency, and shutdown coverage.

### Original plan entry

## Named Codex agents and local session control

Make durable named agents the only identity model for HQ-managed Codex bridges, and make the local HQ daemon the owner and supervisor of every named agent runtime. `hq codex` and the TUI become thin local control-plane clients that ask the daemon to inspect, start, stop, and switch agents between their known Codex threads. Treat agent lifecycle and session assignment as installation-local control-plane state; keep mailbox messages, questions, answers, and relay delivery as the Nostr-carried data plane.

### Required behavior

- Require every `hq codex` invocation to name a durable agent with `--agent NAME`.
  - Reject bare `hq codex`.
  - Remove the anonymous bridge path and the legacy top-level `--resume THREAD_ID` interface.
  - Do not migrate or preserve existing anonymous bridge mailboxes or thread bindings. HQ is still beta; bump or reset incompatible local state as needed.
  - Do not remove generic unnamed harness mailboxes used by `hq ask`, `send`, `poll`, or non-bridge integrations unless they are independently made obsolete. This task is specifically about HQ-managed Codex bridge sessions.
- Turn `hq codex --agent NAME` into a daemon control request rather than a foreground bridge runtime.
  - Ensure the local HQ daemon is running using the same auto-start path as other HQ clients, then submit one idempotent launch request over the local control socket.
  - The daemon owns the named agent's bridge worker and the `codex app-server --stdio` child process. They survive exit of the invoking CLI or TUI and stop with the daemon.
  - `--yolo`, the optional initial prompt, the requested session action, and all other launch options travel in the local request and are applied by the daemon-owned worker.
  - Capture the invoking client's complete environment snapshot and send it transiently with the launch request. Launch the app-server with that snapshot rather than the daemon's startup environment, so credentials, `PATH`, Codex configuration, and other caller-local settings match the shell or TUI that requested the launch.
  - Capture the invoking client's current working directory and use it when `--cwd` is absent. Resolve an explicit relative `--cwd` against that caller directory before sending the request; the daemon validates the resulting absolute directory on the local machine.
  - Wait for a definitive ready or failed result before the CLI exits, print the agent name, selected thread, directory, and runtime status, and leave the worker running after success.
  - Do not fork another `hq` executable beneath the daemon. The supervisor should host the bridge worker in-process and spawn only the Codex app-server child, keeping one lifecycle authority and one local RPC surface.
- Preserve and enforce these invariants:
  - A durable agent has zero current Codex sessions before its first successful thread start, then one current selected session.
  - An offline agent retains its current selection; presence and selection are separate concepts.
  - A Codex thread is permanently bound to at most one mailbox and agent.
  - Selecting or creating another thread changes the single current selection without deleting older bindings.
  - Historical sessions remain available for later resume and cannot be reassigned to another agent.
- Give every newly created Codex thread its durable identity in developer instructions. Compose the existing structured-input instruction with language equivalent to:

  ```text
  You are operating through HQ as the durable agent named "fred".
  This name identifies your HQ mailbox across Codex thread replacements.
  Do not infer personality, permissions, authority, or repository scope from the name.

  When progress requires an answer from the human, use the structured request_user_input tool.
  ```

  Resuming a thread must retain that thread's existing instructions. Add exact protocol tests proving the name is present for new threads and that resume requests do not attempt to replace developer instructions.
- Record enough information for an agent's historical-session chooser:
  - harness and external session or thread ID;
  - the repository context and exact working directory used for that session;
  - creation or first-selection and most-recent-selection times;
  - whether it is the current selection;
  - whether the owning agent is active or offline.
  Existing `harness_bindings` and mailbox-wide contexts do not preserve an unambiguous thread-to-directory association, so introduce an explicit session projection or enrich the signed installation-private selection facts instead of inferring the directory from timestamps.
- Add domain operations and local RPC/client support to list a named agent's sessions and control its local runtime. Keep this separate from storage-only interfaces so SQLite is not responsible for spawning processes. Suggested boundaries:
  - `domain` DTOs and interfaces for session history, runtime state, and start, resume, and stop requests;
  - `domainrpc` methods and `hqclient` implementations;
  - a node-owned supervisor package that runs `codexbridge.Run`, tracks one local worker per named agent, passes the request's environment to `codex app-server`, exposes starting, running, stopping, failed, and offline state, and shuts workers down cleanly with the node.
- Runtime control must be installation-local:
  - no process command, filesystem path, ownership lease, presence, or runtime status is published through Nostr;
  - caller environment snapshots are sensitive, ephemeral control-plane inputs: never put them in canonical events, SQLite, mutation results, the bridge ledger, Nostr, logs, status details, diagnostics, or error strings, and discard them after constructing the child process environment;
  - local RPC retries may identify an environment-bearing launch by request ID and digest, but must not persist or echo the environment itself;
  - durable name and session-selection facts may remain signed installation-private events and rebuildable projections;
  - Nostr remains the data plane for mailbox traffic and relay delivery;
  - document that a future remote controller will command the owning node's control plane, and that paths are interpreted and validated on that node.
- Make runtime commands safe under retries and races:
  - use stable request IDs or idempotent desired-state handling so a lost RPC response cannot launch two bridges;
  - retain the existing named-agent lease as the final exclusion boundary;
  - all new CLI and TUI launches are daemon-owned; reject any conflicting legacy or independently owned lease clearly instead of trying to kill an unowned process;
  - select a session only after `thread/start` or exact `thread/resume` succeeds and returns the requested thread ID;
  - a failed start or resume must leave the prior durable selection unchanged;
  - switching a node-owned live agent must require confirmation, cancel the old bridge cleanly, and report if the requested replacement fails;
  - node shutdown or restart stops supervised workers and leaves their agents offline with selections intact; automatic worker restart is out of scope.
  - support concurrent workers for different named agents without bridge-ledger races or lost checkpoints; use node-owned serialization or independently persisted per-agent and per-thread ledger namespaces instead of allowing several workers to overwrite one shared sidecar file.
- Preserve mailbox routing across rotation:
  - uncorrelated root messages belong to the durable agent mailbox and may be delivered to its currently selected thread;
  - replies correlated to an older Codex thread must not leak into a replacement thread;
  - when that historical thread is selected again, its correlated pending replies become eligible;
  - an unavailable or missing historical Codex rollout produces an actionable error and does not silently select or create a different thread.
- Add an agent and session management flow to the TUI:
  - open a searchable chooser of non-retired named agents;
  - show active or offline state, current thread, and current directory;
  - after choosing an agent, show its current and historical sessions with a clear current marker, shortened thread ID, directory, and useful time metadata;
  - selecting a historical session asks the local control plane to resume that exact thread;
  - include a "new Codex thread" action with a directory input, defaulting sensibly to the agent's current directory or the TUI launch directory;
  - TUI launch and resume requests carry the TUI process's environment snapshot and launch directory under the same transient handling rules as `hq codex`;
  - resolve, clean, and verify that the path exists and is a directory on the controlled node before stopping an existing worker;
  - show starting, running, failed, ownership-conflict, and offline outcomes without freezing the Bubble Tea update loop;
  - preserve the existing inbox selection, drafts, focus, and recipient picker across agent and runtime invalidations.
- Keep CLI and TUI behavior backed by the same daemon-owned lifecycle API. Neither client may run its own bridge or define separate selection, rotation, environment, readiness, or lease semantics.
- Update `README.md`, `docs/design.md`, `docs/events.md`, embedded help, and command summaries:
  - remove anonymous `hq codex` and legacy `--resume` examples;
  - describe `hq codex` as a daemon launch client, including caller environment and working-directory inheritance, ready acknowledgement, detached lifetime, and daemon-shutdown behavior;
  - describe name injection and thread history;
  - describe the TUI controls;
  - explicitly diagram or explain the local control plane versus Nostr data plane;
  - state that no anonymous-data migration is supported.

### Likely implementation areas

- `internal/codexbridge/bridge.go`, `protocol.go`, and bridge and dispatcher tests: require a name, compose developer instructions, support exact named-session resume, and retain correlation isolation.
- `internal/domain/store.go`, `changes.go`, and new control-plane interfaces: add session-history and runtime-control models without conflating persistence and process supervision.
- `internal/event/event.go`, `validate.go`, `reducer.go`, `internal/store/sqlite.go`, and `named_agents.go`: persist and rebuild session-specific context and expose ordered history while preserving unique ownership.
- `internal/domainrpc`, `internal/hqclient`, and local-wire version tests: expose session listing and runtime commands with reconnect, idempotency, transient environment transport, and ready or failed acknowledgement behavior.
- `internal/node` plus a focused supervisor package: own all CLI- and TUI-launched Codex bridge lifecycles, construct app-server child environments, coordinate ledgers, cancellation, status, diagnostics, and node shutdown.
- `internal/tui/tui.go` and `tui_test.go`: implement the agent and session chooser, new-thread directory entry, confirmation, and asynchronous status and error handling.
- `internal/cli/app.go`, CLI and end-to-end tests, help, and documentation: require `--agent`, remove anonymous resume syntax, collect caller launch context, ensure the daemon, submit the control request, and report its result without running a foreground bridge.

### Acceptance criteria

- `hq codex` without `--agent NAME` fails before starting Codex or creating a mailbox.
- `hq codex --yolo --agent bob` auto-starts the HQ daemon when necessary, asks it to launch Bob, waits until Bob's app-server is ready, then exits successfully while Bob remains running beneath the daemon.
- The app-server receives the invoking CLI's environment exactly as the requested child environment and uses the invoking shell's current directory when `--cwd` is absent; a relative `--cwd` is resolved against that directory.
- TUI launches apply the same inheritance rules using the TUI process as the caller.
- Environment values never appear in durable storage, Nostr traffic, logs, diagnostics, status output, or RPC results, including on launch failure and retry.
- A newly created thread for `fred` receives both the durable-name and structured-human-input developer instructions.
- Creating an agent leaves it with zero selected sessions; the first successful start selects one.
- Starting a replacement preserves the previous binding and leaves exactly one current selection.
- A store rebuild returns the same current selection and complete session-specific directory history.
- Attempting to bind another agent to a known thread is rejected.
- The TUI can resume either of two historical threads for one offline agent and can start a new thread in a user-entered valid directory.
- Selection changes only after successful app-server acknowledgement; missing rollouts, invalid directories, process-start errors, and ownership conflicts are visible and non-destructive.
- Switching a live supervised agent is confirmed and cannot result in two workers for the same name.
- Old-thread replies are delivered only when their thread is selected; durable root messages follow the agent's current selection.
- TUI-launched workers continue after the TUI exits, stop with the node, and remain offline instead of auto-restarting after a node restart.
- Two different named agents can run concurrently under the daemon without ledger corruption; repeated delivery of one launch request never creates duplicate workers or app-server children.
- Control operations and runtime state never create Nostr outbox traffic.
- Store, reducer, RPC and client, supervisor, bridge, CLI, TUI, architecture, and relevant end-to-end tests pass.
# 2026-08-24 — Exhaustive project command registry

Introduced a closed, typed registry for all project commands, preserving the existing canonical JSON wire format while centralizing operation identity, codecs, creation/runtime metadata, and local home execution. Replica methods now encode typed commands, canonical ingestion rejects unknown or malformed operations deterministically without mutation, runtime handlers receive decoded typed data, and event validation derives creation semantics from the registry. Added exhaustive codec/executor completeness coverage and an integration test for unknown-command rejection.

### Original plan entry

## Exhaustive project command registry

Unify project command encoding, decoding, home execution, and runtime routing behind a typed command registry. Build this alongside or after the typed event reducer so command results emit registered event types.

Scope:

- Define typed command operations and payload codecs for every project command.
- Register each operation with validation, home-side execution, result handling, and whether runtime or supervisor participation is required.
- Replace string switches distributed across `projects.go`, `project_commands.go`, and `node.go`.
- Make unsupported operations fail explicitly without mutating project state or reporting a committed result.
- Ensure local methods and remote replica methods use the same command definitions and request validation.
- Add completeness tests proving every exported remote-capable project mutation has a codec and home handler.

Primary areas:

- `internal/domain/projects.go`
- `internal/event`
- `internal/store/projects.go`
- `internal/store/project_commands.go`
- `internal/node/node.go`
- `internal/hqclient`
- `internal/domainrpc`

Risks:

- Runtime operations have side effects and saga state; registration must not bypass mutation receipts, stale-head checks, or restart recovery.
- Command compatibility must be maintained for already queued canonical commands.

Acceptance criteria:

- Adding a command requires one typed registration rather than coordinated edits to unrelated string switches.
- Every supported operation round-trips through encode, canonical transport, decode, execute, and result projection tests.
- Unknown commands receive a deterministic rejection and cannot be silently ignored.
- Existing queued commands continue to execute after upgrade.

Implementation plan:

- Define a closed `ProjectCommandOperation` vocabulary and typed command bodies in `internal/domain`, with one registry entry per operation containing its decoder, creation/runtime metadata, and local executor where applicable.
- Make replica-facing project methods encode typed command data through the registry; make arbitrary `QueueProjectCommand` validate and normalize its operation/body before canonical transport while preserving existing JSON wire shapes.
- Replace the store's operation-string execution switch with registry decode/execute. Runtime-required entries pass typed decoded data to a typed runtime handler; unknown or malformed commands publish deterministic rejected results without mutation.
- Replace node runtime string decoding with typed command-data dispatch and preserve stable command IDs, expected heads, saga idempotency, and compatibility for already queued canonical commands.
- Route pending-creation presentation and event validation through registry metadata where layering permits, and add completeness tests covering every exported remote-capable mutation, codec round trips, local execution, runtime routing, and unknown-command rejection.

Risks and decisions:

- Existing command JSON is canonical history, so typed codecs must retain the exact current body shapes rather than introducing envelopes or renamed fields.
- Runtime commands span multiple authoritative transactions; they retain saga-owned idempotency and do not use the single local mutation receipt used by ordinary registered commands.
- The registry lives in the domain layer so store and node share operation identity and codecs without creating package cycles; concrete local execution remains expressed through a focused domain target interface.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Canonical schema-2 message contract and schema-1 legacy adapter

Added harness-neutral typed presentation, correlation, and ordered technical metadata to canonical messages; introduced strict schema-specific decoding, semantic bounds, and final signed-wire enforcement; and isolated all schema-1 structural-line compatibility inside the event projection boundary without rewriting canonical bytes. Extended message projection and SQLite persistence, migrated the primary store, harness, Codex, and TUI paths needed to exercise the contract end to end, and added compatibility, reducer, migration, behavioral, wire-bound, and race coverage.

### Original plan entry

## Canonical schema-2 message contract and schema-1 legacy adapter

Define the versioned canonical message wire contract and the single compatibility boundary that
turns historical structural detail lines into typed projections. This phase deliberately stops at
the event/model projection seam; persistence, producers, and presentation migrate in the following
phases.

Scope:

- Add shared model types for the validated presentation kinds `update`, `final-answer`, `status`,
  and `notice`; harness-neutral provider/session/operation/optional-item/optional-request
  correlation; and ordered namespaced technical sections with stable keys, optional labels, and
  string values.
- Define explicit schema constants and strict, version-specific text payload decoders. Keep the
  schema-1 text payload shape exact, add the schema-2 typed fields, make current inspection and
  reduction accept schemas 1 and 2, and leave schema-1-only inspection able to retain schema-2
  canonical bytes as unsupported.
- Add bounded validation for presentation, correlation combinations and identities, namespaces,
  keys, labels, values, section/field counts, duplicate namespace/key pairs, UTF-8, aggregate
  technical payload size, and the actual signed 64 KiB wire limit including escaped/multibyte data.
- Add a clearly named legacy schema-1 projection adapter in `internal/event`. It alone may parse
  historical `Kind`, `Phase`, harness/Codex correlation, and known project-output provenance lines.
  Scope project provenance recognition by message purpose and exact legacy shape; keep unrelated
  human details, including CLI `--details`, visible and untouched.
- Extend `event.MessageProjection` with typed presentation, correlation, and technical sections.
  Schema-2 messages project those fields directly; schema-1 messages use only the legacy adapter,
  without changing canonical bytes.

Acceptance criteria:

- Schema-2 message payloads strictly validate, sign, inspect, and project with identical typed
  semantics and ordered technical sections.
- Exact schema-1 messages still validate and project through the isolated adapter; schema-1 payloads
  reject schema-2-only fields.
- A schema-1-only reader reports a signed schema-2 message as unsupported while retaining its exact
  canonical bytes; current readers accept both versions.
- Legacy harness presentation/correlation is projected correctly without any model, store, RPC, or
  TUI parser dependency.
- A known schema-1 project-output fixture moves only recognized provenance into a legacy technical
  section, while arbitrary user details with similar words remain human-readable.
- Invalid combinations, duplicates, excessive counts/lengths, malformed UTF-8, and payloads that
  exceed the signed-wire limit after JSON escaping fail closed.

Implementation plan:

- Modify `internal/model/correlation.go` to replace line-oriented correlation parsing with the
  harness-neutral typed identity, and add `internal/model/message_semantics.go` for presentation and
  ordered technical-section DTOs plus focused validity helpers. Update model tests to cover JSON
  shape, valid combinations, and value semantics without parsing `Details`.
- Modify `internal/event/event.go` to introduce schema-1/schema-2 constants, preserve a private exact
  schema-1 payload struct, extend the public schema-2 `TextPayload`, dispatch strict payload decoding
  by content schema, default unrelated content to schema 1, and have current inspection accept both.
- Modify `internal/event/validate.go` to validate schema/type compatibility and every semantic bound,
  then make signing enforce `MaxWireBytes` on the final serialized event rather than relying on
  component limits. Add table-driven failing tests first for unknown fields, invalid correlation,
  duplicate technical fields, escaped/multibyte overflow, and older-reader retention.
- Add `internal/event/legacy_message.go` with a pure `projectLegacyMessage` adapter. It will split only
  exact historical structural lines, preserve human line order/content, recognize project provenance
  only for `project-output`/`system-notice` shapes, and emit stable `hq.legacy.*` sections.
- Modify `internal/event/reducer.go` so schema-2 projects directly and schema-1 delegates to the
  adapter. Add reducer fixtures for harness correlation, Codex aliases, project provenance, ordinary
  lookalike user details, shuffled arrival, duplicate delivery, and unchanged raw bytes.

Tests to add first:

- Event validation tests for every presentation kind, all legal correlation shapes, partial/invalid
  identities, technical ordering, duplicate namespace/key pairs, invalid UTF-8, field/count/aggregate
  bounds, and worst-case JSON escaping under the wire limit.
- Compatibility tests proving strict schema-1 rejection of schema-2 fields, current schema-1/schema-2
  acceptance, schema-1-only unsupported retention, and canonical byte preservation.
- Reducer tests proving direct schema-2 projection and isolated schema-1 conversion, including known
  project output versus user-authored lookalikes.

Risks and decisions:

- Go strings are always byte sequences, so validation must check UTF-8 before byte-length bounds and
  wire tests must measure the fully signed JSON envelope.
- Empty correlation is valid; once any correlation member is present, provider and session are
  required, while operation/item/request remain opaque optional identifiers subject only to bounds.
- Technical sections preserve producer order for display, but duplicate namespace/key pairs are
  rejected globally so consumers never need precedence rules.
- `Details` stays byte-for-byte human content for schema 2. The schema-1 adapter may remove only exact
  recognized legacy structural lines from the projected copy; it never rewrites canonical history.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Typed message projection persistence and RPC round-trip

Completed and proved the typed message round trip across local create/filter/reply, project-input routing, SQLite restart and forced canonical rebuild, schema-1 rebuild compatibility, encrypted peer replication and duplicate delivery, domain RPC, and the HQ client. Empty-correlation replies now inherit the original full correlation while explicit reply correlation remains authoritative; older message JSON without typed fields remains compatible.

### Original plan entry

## Typed message projection persistence and RPC round-trip

Finish and prove the typed message read/write path from local domain clients through canonical
events and disposable SQLite projections. The schema-2 DTOs, projection columns, and primary
writers already exist; this phase makes their round-trip guarantees explicit and closes gaps around
reply inheritance, transport, rebuilds, and RPC compatibility.

Scope:

- Add reusable test fixtures and equality assertions for presentation, full provider/session/
  operation/item/request correlation, ordered technical sections, human `Details`, purpose, and
  context.
- Prove ordinary create/get/list and explicit correlation filters preserve typed values without
  consulting `Details`. Keep the flat harness columns as indexed/backward-compatible read fields,
  but make the typed `Correlation` value authoritative for new writes.
- Make store replies inherit the original typed correlation when the caller leaves correlation
  empty, while preserving an explicitly supplied valid correlation unchanged. Ensure repeated
  payload reconstruction for project routing retains every typed field.
- Prove schema-2 peer transport and duplicate delivery preserve identical typed values and ordered
  technical sections on the receiving installation.
- Prove close/reopen and a forced canonical projection rebuild restore the same typed message value
  from signed bytes, with no dependence on the old projection columns.
- Add domain RPC and HQ client round-trip tests for create, reply, get, list, and conversation history
  responses. Verify strict request decoding still rejects unknown fields and older JSON that omits
  the new fields remains compatible.
- Add an explicit schema-1 canonical fixture to the store rebuild tests, proving only the event-layer
  legacy adapter supplies typed correlation/presentation and that no store parser is involved.

Acceptance criteria:

- Local create, get/list, reply, restart, forced rebuild, peer replication, duplicate delivery,
  domain RPC, and HQ client paths preserve identical presentation, full correlation, technical
  section order/labels/values, human details, purpose, and context.
- An empty-correlation reply inherits all original correlation members; an explicit reply
  correlation is not overwritten.
- Correlation filters continue using indexed provider/session/operation projection columns and work
  when `Details` has no structural lines.
- Schema-1 rebuild compatibility continues to originate only in `internal/event`; arbitrary
  schema-2 human details that resemble legacy lines remain unchanged.
- Existing message JSON without typed fields still decodes, and strict RPC request envelopes still
  reject unknown fields.

Implementation plan:

- Add failing store tests first in `internal/store/sqlite_test.go` for typed create/reply/restart/
  forced-rebuild equality and schema-1 rebuild compatibility, plus transport coverage in
  `internal/store/transport_test.go` for typed replication and duplicate delivery.
- Update `internal/store/sqlite.go` only where those tests expose gaps: centralize canonical typed
  equality helpers in tests, inherit original correlation for empty replies, and ensure every
  project-routing payload reconstruction uses the shared schema-2 payload builder.
- Add focused `internal/domainrpc` service tests that capture typed create/reply requests and return
  typed get/list/history results through the actual local-wire JSON boundary.
- Add focused `internal/hqclient` tests using a real in-memory local-wire server to compare typed
  requests and responses rather than merely checking method names.
- Retain the current flat harness fields and filter DTOs as compatibility/index surfaces for now;
  producer cleanup and any eventual API removal remain in the following phase.

Risks and decisions:

- SQLite JSON decoding commonly turns a nil technical-section slice into an empty slice. Equality
  assertions normalize only this representational difference and remain strict about section and
  field order.
- Canonical timestamps are second-granularity, so fixtures compare semantic message fields rather
  than caller-side subsecond timestamps.
- Peer transport tests must compare the canonical typed projection after unwrap, not encrypted
  wrapper bytes whose recipient-specific envelope is intentionally different.
- Reply inheritance applies only when the caller supplies an empty correlation; explicit values are
  validated at the schema-2 event boundary and remain authoritative.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Typed message producers and behavioral consumers

Migrated the final project message writers to schema 2: project outputs now preserve caller semantics and append ordered typed provenance, while resource-health and pending-work notices use typed notice presentation and project namespaces. Project-output retries reconcile through the authoritative provenance row with strict typed collision checks, the TUI no longer duplicates flat correlation or retains an unused details parser, and architecture coverage guards against new structural Details literals.

### Original plan entry

## Typed message producers and behavioral consumers

Finish the writer migration after an inventory confirmed the remaining schema-1 structural message
authors are confined to project output provenance, project resource-health notices, and closed/
archived project pending-work notices. Generic harness, Codex compatibility, ordinary store/peer,
reply, retry collision checks, and primary TUI compose paths already use typed semantics.

Scope:

- Convert `CreateProjectOutput` to schema 2. Preserve caller presentation, full correlation,
  technical sections, human details, purpose, and context; append ordered diagnostic provenance in
  `hq.project.output_provenance` without copying it into `Details`.
- Preserve the existing authoritative `project_output_provenance` table and actor-label behavior.
  Emit project, assignment, and project-thread IDs for every output; append late/current-assignment/
  current-agent/current-thread diagnostics in stable order only for late output.
- Convert resource-health notices to schema 2 with `notice` presentation and an ordered
  `hq.project.resource_health` technical section. Keep the body human-readable and move project,
  resource, previous/current health, and optional health JSON out of `Details`.
- Convert closed/archived project pending-work notices to schema 2 with `notice` presentation and an
  ordered `hq.project.pending_message` section. Keep project behavior sourced from typed project
  state and acceptance records, never from technical fields.
- Stop TUI compose code from duplicating typed correlation into deprecated flat harness fields and
  delete the now-unused generic `detailValue` parser. Retain read-side flat fields only as the
  compatibility/index surface established in the persistence phase.
- Keep all idempotency/collision checks strict over body, human details, presentation, correlation,
  and ordered technical sections. Add a source-level conformance test preventing new non-legacy
  structural message writers outside `internal/event/legacy_message.go`.

Acceptance criteria:

- Every non-legacy message writer emits schema 2; project writers contain no `Kind`, harness, or
  project-provenance protocol lines in `Details`.
- Project output retains caller typed semantics and existing technical sections, adds stable
  `hq.project.output_provenance`, preserves persistent provenance rows, and marks late output exactly
  as before.
- Project resource-health and pending-work notices present as typed notices with generic technical
  sections and unchanged human-readable bodies.
- Routing, acceptance, late-output classification, actor labels, and project lifecycle behavior are
  invariant when technical keys, labels, or values are not consulted.
- TUI-created messages author only `Correlation`; no normal-path consumer calls a details parser.

Implementation plan:

- Update project tests first to require schema 2, typed presentation/correlation preservation,
  unchanged human details, stable new namespaces/field order, existing-section retention, and the
  unchanged `project_output_provenance` row.
- Refactor `internal/store/project_delivery.go` to build a copied message with appended provenance,
  marshal through `textPayloadForMessage`, and set `MessageSchemaVersion` explicitly.
- Refactor `internal/store/projects.go` and `internal/store/project_inbound.go` notice payloads to
  typed `TextPayload` values with presentation and technical sections, preserving account audience,
  membership parents, body, actor label, timestamps, and project event ordering.
- Remove the TUI's redundant flat-field writes and unused `detailValue` helper; migrate any tests
  that still construct normal-path semantics as structural details.
- Add or extend architecture/conformance coverage that inventories `event.TextPayload` message
  writers and rejects structural `Details` prefixes outside the isolated legacy adapter and
  intentional compatibility fixtures.

Risks and decisions:

- Technical metadata is diagnostic only. The provenance table and typed project state remain the
  source of truth for delivery, assignment, and late-output behavior.
- Appending producer metadata must copy the section slice before mutation so caller-owned values and
  idempotency expectations remain stable.
- Human `Details` are no longer trimmed or augmented by project delivery; schema-2 preserves them as
  supplied, including blank lines and legacy-looking prose.
- Project notice bodies remain visible when technical details are collapsed; identifiers and health
  JSON are disclosed only through the generic technical panel.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Generic technical presentation, documentation, and conformance

Removed the TUI's remaining structural-`Details` parser so human details always render unchanged and all presentation, correlation disclosure, and thread-name annotation use typed message fields. Arbitrary ordered technical namespaces now remain wholly behind `i`, with conformance tests proving label/key order, unknown-namespace rendering, structural-lookalike human text preservation, and behavioral invariance. Documented the schema-2 contract, validation bounds, legacy reducer boundary, harness and project namespaces, SQLite schema 30 round trip, and generic TUI disclosure across the event, harness, project, design, and README surfaces; full tests, vet, compatibility suites, diff checks, and store/bridge/TUI race tests pass.

### Original plan entry

## Generic technical presentation, documentation, and conformance

Finish the schema-2 message work at the presentation and documentation boundary. `Details` is
always human content and must be rendered without parsing or rewriting; presentation, correlation,
technical disclosure, and thread-name annotation must come only from typed message fields. Keep
technical metadata behaviorally inert and render every namespace generically under `i`.

### Implementation plan

1. Make the TUI presentation path entirely typed in `internal/tui/tui.go`.
   - Delete `presentationDetails` and its hard-coded `Kind`, `Phase`, harness, and HQ prefix
     allowlist.
   - Render non-empty `Message.Details` literally in both collapsed and expanded views.
   - Convert `technicalIdentifiers` into an app-aware typed renderer so provider/session
     correlation can resolve a mutable thread name through `threadSessions` while retaining the
     immutable session ID.
   - Preserve the existing derived `hq.message.identifiers` and `hq.message.correlation` groups,
     field order, label-or-key display fallback, arbitrary namespace rendering, and whole-section
     `i` disclosure. The collapsed hint must depend only on typed/derived technical content and
     technical context, never on text inside `Details`.

2. Add presentation and behavior conformance coverage in `internal/tui/tui_test.go`, preferably as
   failing tests before the production edit.
   - Replace the legacy expanded-details annotation test with a typed-correlation test that proves
     the provider/session pair resolves the friendly thread name only in the expanded technical
     block.
   - Prove structural-looking human lines such as `Kind:`, `Harness session:`, and project-like
     labels stay visible unchanged before and after `i`.
   - Prove an unknown namespace renders generically only after `i`, preserves section/field order,
     uses labels only for display, and leaves ordinary details visible.
   - Prove changing technical namespaces, keys, and labels cannot change conversation grouping,
     action-unit grouping, final-answer selection, or reply targeting when typed presentation and
     correlation are unchanged.
   - Retain the existing collapsed-border hint, identifier disclosure, Markdown/body, and human
     details assertions.

3. Strengthen the source-level contract in `internal/architecture/dependencies_test.go`.
   - Add a guard that production TUI code does not contain the historical structural-details
     protocol prefixes, while leaving the isolated `internal/event/legacy_message.go` schema-1
     adapter as the only compatibility parser.
   - Keep the existing producer guard that prevents new text-payload literals from embedding
     structural prefixes in `Details`.

4. Document the complete contract without treating diagnostic metadata as semantics.
   - Update `docs/events.md` with per-event schema support, the strict schema-1/schema-2 text
     payload shapes, typed presentation/correlation fields, technical-section bounds and namespace
     conventions, exact-byte unsupported retention, the isolated legacy projection rule, and the
     64 KiB signed-wire limit.
   - Update `docs/harnesses.md` with typed producer namespaces (`hq.harness.output`,
     `hq.harness.status`, and `hq.harness.request`), opaque provider correlation, reply copying, and
     the rule that human instructions/errors/options remain in `Details`.
   - Update `docs/projects.md` with typed project message semantics, diagnostic provenance and
     notice namespaces, preserved project-output provenance/idempotency, and the prohibition on
     reading technical sections for project behavior.
   - Update `docs/design.md` with SQLite schema 30, typed projection/RPC round trips, schema-1
     compatibility at the reducer boundary, and the semantic-versus-diagnostic architecture rule.
   - Update `README.md` TUI/help text so `i` is described as generic technical disclosure, human
     details remain visible, unknown namespaces need no UI allowlist, and friendly thread names are
     derived from typed provider/session identity.

5. Verify cross-layer conformance and close only regressions caused by this phase.
   - Run focused TUI and architecture tests first.
   - Run `go test ./...`, `go vet ./...`, `git diff --check`, and race tests for
     `./internal/store`, `./internal/harnessbridge`, `./internal/codexbridge`, and `./internal/tui`.
   - Re-run canonical event/store/RPC/client/transport tests that exercise schema-2 validation,
     signing, projection, persistence, rebuild, replication, unsupported schema retention,
     schema-1 compatibility, malformed/duplicate/escaped/multibyte/wire-bound rejection, and typed
     JSON round trips. No canonical history or schema-1 payload is rewritten by this phase.

### Risks and decisions

- `Details` can legitimately begin with historical protocol-looking words. The new contract favors
  preserving that human text; only schema-1 projection may have already separated recognized
  producer-shaped legacy lines before they reach the TUI.
- Thread names are mutable display metadata. They must never be copied into immutable messages or
  used as correlation identity; the typed provider/session pair remains authoritative.
- Built-in identifiers and repository/source context are derived technical display groups rather
  than serialized technical sections. They remain inert and hidden with the same `i` control.
- No schema or producer changes are planned here: those paths were implemented and tested in the
  preceding stacked phases. Any missing cross-layer behavior found during verification will be
  fixed in the owning package and documented before completion.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Typed canonical message semantics final integration audit

Audited the complete typed-message umbrella against the four preceding stacked phases and mapped every schema, projection, persistence, RPC, replication, legacy, producer, presentation, validation, wire-bound, and behavior requirement to passing coverage. Closed the one residual gap by removing TUI fallbacks from typed `Correlation` to deprecated flat harness fields: replies now copy the typed object directly, conversation keys use typed provider/session identity, and action units use typed operation identity. Added regression coverage proving flat-only fields cannot merge conversations, choose an action, or leak into replies, while conflicting flat fields cannot override valid typed correlation. Full tests, fresh compatibility suites, vet, diff checks, and store/harness/Codex/TUI race suites pass.

### Original plan entry

## Typed canonical message semantics and technical metadata — final integration audit

Message `Details` currently mixes human-readable supplementary content with machine-readable fields. Canonical reduction parses harness correlation back out of line-oriented text; the TUI separately parses presentation kind, correlation, request identity, and visibility using hard-coded key prefixes. Project delivery adds another set of raw keys. This makes display labels an implicit cross-module protocol.

Make message structure explicit: behaviorally meaningful data travels through typed canonical fields, while diagnostic/display-only key/value data travels through namespaced technical sections. Message body and human details must never be parsed to recover structure or drive behavior.

Implement the following:

- Introduce canonical message semantics shared through the event, projection, model, store, RPC, client, and TUI paths:
  - a validated presentation-kind enum for `update`, `final-answer`, `status`, and `notice`;
  - harness-neutral correlation containing provider, session, operation, optional item, and optional request IDs;
  - namespaced technical sections containing stable machine keys, optional display labels, and string values.
- Treat the technical-section container as always technical. The TUI must hide or show entire sections with `i` without inspecting namespaces, keys, labels, or values. Namespaces identify provenance; keys identify fields; labels are presentation only. Do not add downstream checks such as `key == "Project"` or producer-specific prefix allowlists.
- Reserve typed semantic fields for anything used by routing, conversation identity, grouping, ordering, reply/archive selection, request correlation, final-answer selection, authorization, or other behavior. Generic technical metadata must remain inert: no code may read it to make domain decisions. Promote any value that later becomes behavioral into a dedicated typed field.
- Define bounded validation for presentation kinds, correlation identities, namespaces, keys, labels, values, section and field counts, UTF-8, duplicate namespace/key pairs, and aggregate payload/wire size. Keep provider values opaque and harness-neutral; do not introduce Codex protocol terms into canonical or domain types. Technical sections are display disclosure, not an access-control or secret-storage mechanism.
- Add explicit schema-version support for the extended text payload:
  - retain an exact schema-1 decoder;
  - add a schema-2 text payload carrying typed semantics and technical sections;
  - make new message writers explicitly emit schema 2 while unrelated event types may remain schema 1;
  - have current readers accept schemas 1 and 2 with strict, version-specific payload decoding;
  - ensure older binaries retain schema-2 bytes as unsupported events rather than treating an added schema-1 field as invalid;
  - do not rewrite canonical history.
- Centralize schema-1 compatibility parsing at the canonical projection boundary in a clearly named legacy adapter. It may decode the historical `Kind`, `Phase`, Codex/harness correlation, and project-provenance lines into typed projections and legacy technical sections. No store query, RPC client, TUI code, or new writer may call that parser. Scope legacy project decoding by known message purpose and shape so ordinary user-supplied details are not casually reclassified.
- Extend `event.MessageProjection`, `model.Message`, and the SQLite message projection with typed presentation/correlation fields and technical-section JSON. During full rebuild:
  - schema-2 messages project directly from typed payload fields;
  - schema-1 messages use only the legacy adapter;
  - raw canonical bytes remain untouched;
  - recognized legacy technical lines are presented through technical sections rather than leaking into always-visible human details.
- Remove normal-path uses of `model.ParseMessageCorrelation`, `detailValue`, presentation-kind text parsing, and the TUI technical-prefix list. Delete or confine those helpers to the schema-1 adapter after all consumers use projected typed data.
- Update every message producer, including generic harness output/status/questions/notices, Codex compatibility paths, project output provenance, project system notices, TUI-created replies and session-targeted messages, peer/account message creation, and retry/reconciliation comparisons:
  - populate typed semantics directly;
  - put only human-readable instructions, errors, choices, schemas, and explanations in `Details`;
  - emit diagnostic attributes in stable namespaces such as `hq.harness.output`, `hq.harness.request`, and `hq.project.output_provenance`;
  - do not duplicate message ID in metadata when the canonical/model message ID already supplies it;
  - copy typed harness correlation onto replies instead of serializing and reparsing it.
- Preserve existing project-output provenance persistence. Its technical section represents display/diagnostic provenance, while any field needed for project behavior remains in typed project state or is promoted to a dedicated typed message field rather than read from metadata.
- Update output idempotency and collision checks so typed semantics and technical sections participate where relevant. Ensure repeated payload construction in store routing paths cannot accidentally drop the new fields.
- Persist and scan the new projection columns/JSON through SQLite migration and rebuild, and verify domain RPC and client serialization round-trip them without converting them back to text.
- Render expanded technical sections generically with their namespace visible, preserving field order and using labels only for display. Continue showing built-in message/event/thread/installation identifiers under `i`, grouped under explicit derived HQ namespaces. Thread-name annotation must use typed provider/session identity.
- Keep schema-1 human details readable, preserve CLI/user-supplied `--details` as human content, and document that `Details` is not a structural channel.
- Update `docs/events.md`, `docs/harnesses.md`, `docs/projects.md`, `docs/design.md`, and relevant README TUI/help text with the schema-2 message contract, namespace conventions, typed-versus-technical boundary, legacy compatibility rule, and `i` behavior.

Expected implementation areas include:

- `internal/event/{event.go,validate.go,reducer.go}` and version/compatibility tests;
- `internal/model/{message.go,correlation.go}` or replacement semantic types;
- `internal/store/{sqlite.go,transport.go,project_delivery.go,projects.go,project_inbound.go}` plus migration/rebuild tests;
- `internal/harnessbridge/{events.go,questions.go,bridge.go}` and remaining Codex adapter compatibility paths;
- `internal/domainrpc`, `internal/hqclient`, and JSON compatibility tests;
- `internal/tui/{tui.go,markdown.go}` and presentation/reply/grouping tests;
- canonical event, harness, project, design, and TUI documentation.

Completion requires tests proving:

- schema-2 messages validate, sign, project, persist, RPC-round-trip, replicate, and rebuild with identical typed semantics and technical sections;
- schema-1 messages still project with correct correlation and presentation through the isolated legacy adapter;
- a schema-1 project-output fixture hides legacy project provenance until `i`, while arbitrary user details using similar words remain visible;
- an older schema-1-only reader classifies schema-2 messages as unsupported and retains their canonical bytes;
- shuffled arrival, duplicate delivery, restart, and full rebuild produce equivalent typed message projections;
- provider/session collisions remain isolated without consulting `Details`;
- final-answer selection, conversation grouping, action-unit grouping, request reply targeting, and reply correlation work when `Details` contains no structural lines;
- changing a technical key or label cannot change behavior, and unknown namespaces render generically only when technical details are expanded;
- project and harness producers no longer serialize structural correlation or presentation kind into `Details`;
- malformed, duplicate, or oversized technical sections and invalid correlation combinations fail validation, including worst-case escaped and multibyte payloads under the signed-wire limit;
- existing human details, approvals, validation errors, options, and schemas remain visible and usable;
- `go test ./...`, relevant store/TUI/harness race tests, `go vet ./...`, and `git diff --check` pass.

### Final integration audit execution plan

The four preceding stacked phases implemented the schema/model/legacy boundary, projection and RPC
round trip, producer migration, and generic presentation/documentation work. This final pass will
not duplicate those changes. It will reconcile every completion criterion above against the
current tree and close the one residual normal-path fallback found during source inspection.

1. Make typed correlation authoritative for TUI behavior in `internal/tui/tui.go`.
   - Remove `correlationForMessage` and its fallback from empty `Message.Correlation` to deprecated
     flat `HarnessProvider`, `HarnessSessionID`, and `HarnessOperationID` projection fields.
   - Have reply creation copy `answerQ.Correlation` directly, conversation grouping read
     `Message.Correlation` directly, and action-unit grouping use only
     `Message.Correlation.OperationID` before the ordinary causal thread/message fallback.
   - Keep the flat SQLite columns and model JSON fields for indexed queries and older local-wire
     compatibility; current store scans already hydrate `Correlation`, so presentation code does
     not need a second authority.

2. Add regression coverage in `internal/tui/tui_test.go`.
   - Prove flat-only harness fields cannot merge distinct causal conversations, choose a harness
     action unit, or leak into a TUI-created reply.
   - Prove messages with identical typed provider/session/operation correlation still group and
     target replies identically even when their deprecated flat fields conflict.
   - Retain the existing provider-collision, final-answer, request-target, technical-invariance,
     structural-lookalike, and unknown-namespace tests as the behavioral contract.

3. Audit the full umbrella acceptance matrix without speculative rewrites.
   - Confirm production `TechnicalSections` reads are limited to validation, persistence,
     equality/idempotency comparison, and generic rendering; producer-specific project behavior
     must remain in typed project state.
   - Confirm all structural text parsing is isolated in `internal/event/legacy_message.go`, all
     current text-message writers select schema 2, and store/RPC/client/project/peer paths preserve
     typed fields.
   - Map schema/sign/project/rebuild/replication/unsupported-reader, schema-1 project-shape,
     provider collision, producer, human-details, malformed/bounds, escaped/multibyte wire, and UI
     disclosure requirements to focused tests. Add a missing test only if the behavior is not
     already proved compositionally.

4. Verify and commit the audit closure.
   - Run focused TUI and architecture tests plus fresh event/store/domain-RPC/client/transport
     compatibility suites.
   - Run `go test ./...`, `go vet ./...`, `git diff --check`, and race tests for
     `./internal/store`, `./internal/harnessbridge`, `./internal/codexbridge`, and `./internal/tui`.
   - Commit with Conventional Commits, remove this entire umbrella entry from `PLAN.md`, and append
     the actual audit summary plus this complete pre-work entry verbatim to `COMPLETED.md`.

### Risks and decisions

- Deprecated flat message fields remain serialized for compatibility, but they are derived/indexed
  projections rather than an independent semantic source. Removing their TUI fallback may change
  hand-constructed legacy JSON that omits `Correlation`; supported store/RPC reads populate the
  typed object, and the old client decoder already treats absent typed fields as non-semantic.
- Equality checks may inspect technical sections to reject a deterministic-ID collision. That is
  content integrity, not a domain decision derived from a particular namespace/key/value, and must
  continue comparing the complete ordered value.
- The schema-1 adapter intentionally parses historical details. No new parser or canonical rewrite
  is permitted during this audit.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Canonical harness activity event and deterministic reducer

Added the schema-2 `harness.activity` event with harness-neutral typed correlation, activity kind/status/content, occurrence time, runtime lifetime, and source sequence under explicit UTF-8, identity, shape, payload, and signed-wire bounds. Activity is restricted to installation-private or active-account audiences from a non-human source mailbox; peer/public/recipient forms and revoked or unrelated account sources fail closed, while schema-1-only readers retain authentic local and account-addressed bytes as unsupported. The pure reducer now produces full-mailbox/provider-isolated logical activity projections and a stable causal message/activity conversation order, coalescing repeated snapshot/item keys without altering any message or thread state. All kinds, strict decoding, malformed shapes, escaped/multibyte bounds, authorization, unsupported compatibility, collisions, duplicates, shuffled arrival, causal order, and message invariance are covered; full tests, vet, diff checks, and event/domain races pass.

### Original plan entry

## Canonical harness activity event and deterministic reducer

Define the signed harness-neutral activity contract and its pure canonical projection before any
store writer begins emitting it. This phase establishes validation, authorization, compatibility,
stable identity, and order-independent reduction while leaving the current local SQLite activity
writer and read API operational until the following phase.

### Implementation plan

1. Extend the shared activity model in `internal/domain/harness_activity.go` and canonical schema in
   `internal/event/event.go`.
   - Add canonical event ID, originating full mailbox address/installation identity, runtime
     lifetime ID, provider event sequence, and stable canonical display-order information to the
     projected activity without turning it into a message or inbox action.
   - Add `TypeHarnessActivity` under schema 2 and a strict `HarnessActivityPayload` containing the
     existing harness-neutral kind/status/title/body/truncation/occurrence fields plus the shared
     typed provider/session/operation/item correlation, runtime lifetime, and source sequence.
   - Keep provider/session/operation/item values opaque. Do not add Codex methods, JSON-RPC data,
     raw provider payloads, request correlation, or technical message sections.
   - Define explicit UTF-8 and byte/count bounds that leave enough envelope space below the actual
     64 KiB signed-wire limit; keep the final wire-size check authoritative.

2. Validate scope, shape, and kind-specific semantics in `internal/event/validate.go`.
   - Accept activity only as schema 2, installation-private or account-addressed, with exactly one
     originating sender mailbox and no recipient/thread ID. Reject peer-addressed and public forms.
   - Require provider/session/operation identity, runtime lifetime, positive source sequence, and a
     valid occurrence time. Require an item for command/file/tool/progress and forbid it for
     operation/plan/diff snapshots.
   - Preserve existing status/body/title rules: operation needs status; plan/diff/progress need
     bodies; command/file/tool need titles and terminal status. Validate printable identities,
     UTF-8, timestamps, truncation state, and exact payload/wire bounds.
   - Extend account authorization so an active account device may publish activity only from its
     own non-human mailbox into the named account audience. Local-root installation-private
     activity remains local; revoked/unrelated, peer, recipient-addressed, and public activity fail
     closed.

3. Add deterministic activity projection in `internal/event/reducer.go`.
   - Add a `HarnessActivityProjection` and `State.HarnessActivities` keyed by the full originating
     mailbox plus provider/session/operation/kind/item logical key, preventing provider or mailbox
     collisions.
   - Project only authorized, causally usable events and retain the winning canonical event ID,
     exact sender, account audience, typed correlation, source lifetime/sequence, signed occurrence
     time, and canonical display position.
   - Apply activities in canonical display order so operation/plan/diff snapshots and repeated item
     keys are deterministic latest-wins projections independent of receipt order or duplicate
     delivery. Completed command/file/tool keys and distinct progress items remain separate rows;
     canonical records remain retained even when a logical projection is superseded.
   - Do not change message/thread projection, inbox state, final-answer selection, or legacy message
     behavior.

4. Add focused contract and reducer tests in `internal/event/event_test.go` and
   `internal/event/reducer_test.go`.
   - Cover every activity kind through validate/sign/inspect/project and assert exact typed values,
     full mailbox association, event ID, source sequence, and deterministic order.
   - Cover missing/invalid identities, kind/status/title/body/item combinations, UTF-8, oversized
     title/body/identities, escaped and multibyte signed-wire boundaries, invalid occurrence time,
     zero sequence, and strict unknown fields.
   - Prove shuffled arrival and duplicates yield byte-for-byte equivalent projections; repeated
     logical keys coalesce while distinct provider/session/mailbox/item identities stay isolated.
   - Prove schema-1-only readers retain schema-2 activity bytes as unsupported and prove local,
     active-account, revoked/unrelated-account, peer, recipient, and public authorization outcomes.
   - Assert activity cannot alter message or thread projections.

### Risks and decisions

- Source sequence is provider-runtime evidence, not a global clock. Canonical causal/display order
  chooses projection winners; lifetime and sequence are retained and validated for deterministic
  same-source ordering and diagnostics, not compared across unrelated runtimes.
- The current unsigned `harness_activities` table remains untouched in this phase. The next phase
  will migrate it to a disposable canonical projection and intentionally discard legacy unsigned
  rows rather than manufacture signed history.
- Dynamic truncation belongs at the canonical authoring boundary in the store/bridge phase. This
  phase defines safe maxima and rejection behavior so callers cannot bypass final wire validation.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-25 — Canonical activity SQLite authoring, privacy, and rebuild

Canonical harness activity authoring now validates typed correlation plus runtime/sequence identity, dynamically fits escaped UTF-8 against the signed 64 KiB envelope, signs account-addressed schema-2 events, and uses ordinary canonical ingest and fanout. Schema 31 discards unsigned legacy rows and rebuilds a source-complete, canonically ordered projection with deterministic 200-row progress retention; same-time reducer ordering honors occurrence and provider sequence. The legacy read API now exposes event, source, audience, correlation, runtime, sequence, and display metadata, while the public write RPC/client API has been removed. Exact replay, migration, restart/rebuild, retention, message invariance, account gift-wrap convergence, full-suite, vet, and race coverage pass.

### Original plan entry

## Canonical activity SQLite authoring, privacy, and rebuild

Replace the unsigned local activity mutation with a store operation that validates, dynamically
fits, signs, authorizes, appends, fans out, and projects canonical activity. Migrate the disposable
SQLite table to event/source/order columns, discard legacy unsigned rows, rebuild solely from the
canonical log, implement deterministic snapshot/progress retention, and prove restart, rebuild,
duplicate, shuffled, cross-device, revoked/unrelated/peer/public, and 64 KiB behavior. Keep the
legacy read API temporarily while removing or narrowing public write RPC access.

Scope:

- Make `UpsertHarnessActivity` an internal producer operation that converts the domain value to a
  schema-2 `harness.activity` content, signs it with the local installation identity, and appends it
  through the ordinary canonical ingest transaction. Replays with identical source identity,
  sequence, occurrence time, and content must produce the same event ID.
- Require full typed correlation plus runtime-lifetime identity and provider sequence at the
  canonical authoring boundary. Preserve the flat harness/session/operation/item fields only as a
  temporary read-side compatibility view. Give the existing bridge the minimum metadata plumbing
  needed to satisfy that boundary; bounded lossless buffering remains in the following task.
- Address activity to the active local human account and use its membership parents so normal
  outbox fanout synchronizes it to active devices. Allow installation-private content only through
  canonical ingest for genuinely local-only callers; the producer operation must never create
  peer-addressed or public activity.
- Dynamically fit title/body text by UTF-8 boundaries against the actual signed 64 KiB envelope,
  retaining kind-specific presentation limits and setting `truncated` whenever fitting changes the
  input. Fail if required metadata alone cannot fit or validate.
- Replace the legacy `harness_activities` table with a disposable canonical projection containing
  event ID, full source installation/mailbox identity, typed correlation columns, runtime/sequence,
  occurrence time, and canonical display order. The logical-key uniqueness must include source
  installation and mailbox as well as provider/session/operation/kind/item.
- Schema migration must drop legacy unsigned activity rows, create the canonical projection shape,
  and invalidate the projection checkpoint. Full projection rebuild must clear the table and insert
  only reduced canonical activities in reducer order.
- Retain only the canonical latest-wins snapshot per reducer logical key and, after projection,
  deterministically retain the newest 200 progress rows per full source/provider session using
  canonical display order rather than receipt time. Keep the query cap at 1,000 chronological rows.
- Extend the temporary read filter with optional source installation identity, populate canonical
  event/source/correlation/runtime/sequence/display fields, and derive legacy flat fields from the
  typed correlation rather than storing competing semantics.
- Include activity invalidations in canonical append/inbound paths without changing message,
  inbox, unread, archive, reply, draft, delivery, or project behavior.
- Remove `activity/upsert` from the public domain RPC protocol and HQ client while retaining the
  read RPC. The in-process bridge/store writer contract remains narrow and daemon-internal.

Implementation plan:

- Add a schema migration and base schema for the canonical activity projection, including source-
  complete uniqueness and query/progress indexes; explicitly discard the version-30 unsigned rows.
- Rework store authoring normalization to accept typed source metadata, choose account audience and
  membership parents, sign/fits-check iteratively at UTF-8 boundaries, and append through canonical
  ingest so authorization, fanout, reduction, mutation receipts, and invalidations share one path.
- Add runtime identity and provider sequence to bridge-produced activity values without changing
  the existing queue/drop policy in this phase.
- Project `event.State.HarnessActivities` during every rebuild, insert rows by
  `HarnessActivityOrder`, prune progress deterministically, and update the legacy list query to
  return canonical fields and order.
- Remove the public write method, request type, server dispatch, client method, and compatibility
  expectations, leaving list compatibility intact.
- Replace unsigned-table tests with canonical authoring, exact replay/duplicate, restart/rebuild,
  shuffled ingest, source isolation, account fanout/privacy, migration-discard, retention/order, and
  escaped/multibyte signed-wire tests. Assert byte-equivalent projections and message-state
  invariance where appropriate.

Risks and decisions:

- The reducer, not SQLite conflict timing, remains authoritative for coalesced winners. SQLite only
  materializes its selected entries and applies the documented bounded progress projection.
- Canonical events remain retained even after progress rows are pruned from the disposable table;
  a rebuild must deterministically reproduce the same bounded table.
- Signing time is the normalized occurrence time so an identical producer replay is idempotent.
  The bridge's strictly increasing provider sequence and per-runtime identity distinguish distinct
  values that otherwise share a logical key.
- Account membership parents are resolved at authoring time. Revoked and unrelated senders remain
  rejected by canonical authorization, and peer/public attempts are covered at the ingest boundary.
- The next phase owns queue backpressure, coalescing, shutdown draining, and serialized relative
  output/activity ordering; this phase must not silently redesign those behaviors.

Acceptance criteria:

- Every bridge-supported kind authors a projected schema-2 event whose event ID and full source
  metadata survive restart and complete projection rebuild.
- Identical replay and duplicate ingest are idempotent; shuffled canonical arrival produces the
  same rows and display ordering; provider and source-mailbox collisions do not merge.
- A version-30 database loses unsigned activity rows by design and reconstructs only signed rows
  from `canonical_events`.
- Two active account devices receive and project account-addressed activity through ordinary
  outbox/inbound handling, while revoked, unrelated, peer-addressed, and public attempts fail
  closed.
- Worst-case escaped and multibyte title/body input is valid UTF-8, explicitly marked truncated
  when changed, and signs below the 64 KiB wire limit.
- Progress retention is exactly the latest 200 reducer-ordered values per full source/provider
  session after live append, restart, and rebuild, without deleting canonical events.
- The legacy read API returns at most 1,000 chronological canonical projections. No public write RPC
  remains, and existing message-only state and behavior are unchanged.
- Relevant store, bridge, RPC, client, event, and transport tests pass under normal and race runs;
  `go test ./...`, `go vet ./...`, and `git diff --check` pass.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## 2026-08-25 — Lossless bounded bridge activity persistence and canonical ordering

Replaced the drop-on-full bridge channel with a mutex-protected 64-item FIFO/coalescing buffer. Durable output, terminal status, and completed command/file/tool work now backpressure until accepted; running/plan/diff/progress snapshots replace the same pending logical key at the tail, while new keys backpressure and cancellation unblocks waits. One relay-wide timeline is assigned before buffering so ready, output/activity, successive work, and stopped status retain deterministic order; persistence uses a relay-owned cancellable context and normal shutdown drains accepted work. Tests exceed capacity, prove lossless terminal delivery, bounded latest-snapshot coalescing, tail ordering, cancellation, output/activity timing, partial-write restart reconciliation, rebuild stability, and race safety.

### Original plan entry

## Lossless bounded bridge activity persistence and canonical ordering

Replace the bridge's drop-on-full persistence queue with one serialized canonical output/activity
path. Durable terminal and completed records apply cancellation-aware backpressure; replaceable
plan/diff/running/progress values coalesce by logical key, and a full buffer with a new key applies
backpressure. Preserve output/activity relative order, drain accepted durable/latest coalesced work
on shutdown, retain transient provider noise only ephemerally, and add overload, cancellation,
reconciliation, deterministic-ID, and race tests beyond the current 64-entry capacity.

Scope:

- Replace the channel/default-drop queue with an explicitly bounded FIFO/coalescing buffer shared
  by canonical assistant output and canonical harness activity. A work item normalized from one
  provider event remains indivisible and publishes output then activity in that stable order.
- Treat assistant output, failed/interrupted status output, terminal operation activity, and
  completed command/file/tool activity as durable. Enqueue them with backpressure until capacity
  is available or the relay ingestion context is canceled; never drop an accepted durable item.
- Treat running operation status, plan, diff, and progress as replaceable snapshots. When the same
  full provider/session/operation/kind/item logical key is pending, remove the older value and append
  the newer value at the tail so its position reflects provider event order. If no matching key is
  pending and the buffer is full, apply the same cancellation-aware backpressure as durable work.
- Keep the buffer bounded by pending logical work, excluding the one item currently being
  persisted. Replacement must not grow memory, and a key that is already in flight may produce one
  persisted intermediate value followed by the latest pending value; canonical reduction still
  selects the later source sequence.
- Continue discarding token deltas, spinners, raw reasoning/model payloads, and all provider events
  that normalize to neither supported output nor supported activity before they reach the buffer.
- Allocate canonical authoring times from one relay timeline. Preserve provider occurrence order,
  make bursts monotonic at signed-second granularity, and give output/activity from the same event
  deterministic adjacent positions. Store those times on normalized work before it can wait or be
  coalesced so retries and delayed persistence cannot consult receiver clocks.
- Make output and activity persistence use a relay-owned context. Parent/worker cancellation stops
  intake and unblocks enqueue waits, while an orderly provider shutdown closes intake and drains all
  accepted FIFO/latest coalesced work. A shutdown-time persistence cancellation records a relay
  failure instead of hanging silently.
- Preserve output reconciliation: stable output IDs plus the delivery ledger must avoid duplicate
  messages, and replay after a partial output-then-activity failure must reconcile the output before
  retrying the activity. Canonical activity IDs remain deterministic for identical runtime,
  sequence, occurrence, content, and membership state.

Implementation plan:

- Add a small mutex-protected `eventBuffer` with bounded items, close/wakeup signaling, contextual
  enqueue, FIFO dequeue, and replace-in-tail behavior. Keep it local to `internal/harnessbridge` and
  test it through relay behavior rather than exporting queue mechanics.
- Classify normalized activity by durability and generate a source-complete coalescing key only for
  replaceable activity-only work. Any work containing canonical output is durable.
- Refactor relay startup, ingestion, publication, and shutdown around the buffer and separate intake
  and persistence cancellation. Pass the persistence context through message reads/creates,
  project-output creation, synchronization, and canonical activity authoring.
- Replace independent output/activity clock allocators with one relay-wide monotonic timeline and
  preassigned output `CreatedAt`/activity `OccurredAt` values, preserving ready/work/stopped order as
  well as the relative order of work that waits or is coalesced before persistence.
- Replace the drop-on-saturation test with overload tests exceeding 64 entries for terminal and new-
  key work, same-key plan/progress coalescing tests, cancellation-unblocks-backpressure tests, and
  shutdown drain tests. Add ordering, partial-failure reconciliation, deterministic-ID/replay, and
  race coverage.

Risks and decisions:

- Coalescing moves a replacement to the buffer tail. Updating in place would let a newer provider
  event jump ahead of durable work that arrived between the two snapshots and would violate the
  serialized source order.
- Capacity bounds pending work, not persisted canonical history. Once a replaceable item is in
  flight it is accepted and may persist; a newer same-key value is a distinct pending item and the
  reducer's sequence-aware latest-wins rule handles both.
- Store persistence is serialized but output plus activity are not one SQLite transaction because
  they use existing message and activity authoring APIs. Stable IDs and replay reconciliation are
  therefore required at the boundary between the two writes.
- The provider event stream is closed by `Instance.Shutdown` before normal relay teardown. Intake
  cancellation is the exceptional escape hatch for a producer blocked on a full buffer, not the
  normal mechanism for declaring accepted work drained.
- A forced persistence cancellation can only prevent an indefinite process hang; it must surface as
  relay failure. Tests for orderly shutdown release persistence and require every accepted durable
  and latest coalesced value to be present before `Done` closes.

Acceptance criteria:

- More than 64 terminal operation and completed command/file/tool records block the producer while
  persistence is unavailable and all appear after persistence resumes; none are silently dropped.
- Bursts of more than 64 plan/progress updates for one logical key remain bounded and persist the
  latest accepted source sequence/value. Bursts of distinct replaceable keys backpressure at
  capacity rather than allocating or dropping.
- Canceling relay intake unblocks a producer waiting for capacity. Normal instance shutdown drains
  all work already accepted into the buffer, including the latest pending coalesced value.
- Output and activity from one provider event, and work from successive provider events, have the
  same deterministic canonical order after live projection, restart, and full rebuild.
- Replaying identical output/activity does not duplicate output messages or canonical activity;
  replay after an injected activity failure reconciles the existing output and eventually persists
  activity without an ID collision.
- Transient unsupported provider events create no queued or canonical work. Existing operation
  tracking, message presentation/correlation, project-output routing, and activity bounds remain
  unchanged.
- `go test ./...`, `go vet ./...`, `git diff --check`, and race tests for the bridge/store/event paths
  pass.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## 2026-08-25 — Unified canonical conversation history and TUI timeline

Added a validated domain-level message/activity union and a strict, canonically ordered paged read without changing the legacy message-only history API. SQLite now persists reducer display order for messages, rebuilds it through schema version 32, and returns projected mixed history isolated by mailbox/provider/session with thread fallback remaining message-only. The new RPC and HQ client method preserve complete typed message and activity fields and fail cleanly against older servers. The TUI now loads one authoritative entry sequence, derives compatibility slices for message-only actions and activity cards, renders reducer order rather than timestamps, and keeps activity out of inbox, unread, reply, archive, draft, delivery, final-answer, and scroll-anchor behavior. Store/RPC/client/TUI tests cover strict pagination, restart/rebuild, coalescing/retention, compatibility, canonical ordering, cache invalidation, and logical anchoring; full test, vet, diff, and race suites pass.

### Original plan entry

## Unified conversation history, RPC compatibility, and TUI activity timeline

Add a typed `ConversationEntry` message/activity union with stable canonical order while retaining
legacy message-only reads. Round-trip it through domain RPC and the HQ client, then move the TUI to
the unified history without making activity an inbox/unread/reply/archive/draft/final-answer target.
Preserve activity card disclosure, logical-message scroll anchoring, 1,000-row query caps, 200
progress projections, provider/session isolation, older-client unsupported behavior, and add
message-only behavior-invariance plus timeline/restart/resize/race coverage.

Scope:

- Add a domain-level discriminated `ConversationEntry` union because domain activity already
  depends on model correlation types. Each entry carries exactly one full `model.Message` or
  `HarnessActivity`, its stable canonical event ID/display order, and an explicit kind; add a paged
  response using the existing `model.ConversationHistoryFilter` and conversation key.
- Preserve `ListConversationHistory` and `conversation/history` byte-for-byte as the legacy
  message-only read. Add a separate unified store operation, RPC method/request, and HQ client
  method so older clients continue decoding and retaining activity only through canonical
  unsupported-event compatibility, without receiving a changed response shape.
- Persist reducer display order on message rows via a schema migration and rebuild. Activity rows
  already carry the same reducer order; unified pagination must use `(display_order,event_id)` rather
  than timestamps, row IDs, or receipt order.
- Query messages in either direction between human and the selected counterparty. Include activity
  only for a provider/session conversation and match the same counterparty mailbox plus exact
  provider/session namespace. HQ thread-fallback conversations remain message-only.
- Cap each page through existing page-limit rules, validate opaque cursors strictly, return entries
  chronologically, and preserve the legacy 1,000-row activity query cap and 200-progress projection.
- Move TUI detail loading to the unified endpoint. Retain derived message/activity slices only for
  existing action, cache, and rendering code, while storing the ordered entry sequence as the
  authoritative timeline so rendering no longer re-sorts by occurrence timestamps.
- Keep conversation summaries, inbox/open/unread counts, latest/final-answer selection, reply and
  archive targets, drafts, delivery state, and compose behavior exclusively message-driven. An
  activity entry has no action ID and cannot become a logical-message scroll anchor.
- Preserve collapsed/expanded activity cards, failure/truncation disclosure, viewport-based toggle,
  resize behavior, and logical message scroll anchoring when activities are inserted, coalesced, or
  re-ordered by a rebuild.

Implementation plan:

- Add conversation entry/page types and the unified method to domain/store contracts, with helpers
  that validate the discriminated union and split entries into compatibility slices where needed.
- Migrate SQLite to add `display_order` to messages, fill it from `event.State.DisplayOrder` on every
  rebuild, and add a unified candidate query over message/activity projections. Page by canonical
  order and hydrate exact typed message/activity values without reconstructing semantics from body
  or `Details`.
- Add `conversation/entries` protocol dispatch and HQ client support; retain and test the legacy
  method and activity-list method unchanged.
- Add an authoritative entry-history map to the TUI load/update/group path. Render entry order
  directly, derive message/activity slices for existing action and cache consumers, and leave a
  compatibility fallback only for hand-built test groups without typed entries.
- Add store tests for mixed pagination/order, restart/rebuild, duplicate/coalesced activity,
  provider/session/source isolation, thread fallback, and legacy message-history invariance. Add
  RPC/client round trips and TUI timeline/action/anchor/resize/cache/race tests.

Risks and decisions:

- Message IDs and event IDs differ for schema-2 messages. Unified cursors and ordering use canonical
  event IDs, while hydrated messages retain their public message IDs for all actions.
- Coalesced activity rows mean canonical history is richer than the bounded disposable unified
  projection. This endpoint intentionally reflects projected conversation history: superseded
  snapshots and pruned progress remain in the canonical log but do not reappear in the TUI.
- A message and activity may share a signed second; reducer `display_order`, not local timestamps,
  is authoritative. Timestamps remain presentation metadata only.
- Adding a required domain store method intentionally makes new in-process implementations explicit;
  old wire clients remain compatible because the legacy RPC method and response do not change.
- TUI caches compare ordered entries as well as derived slices. Activity expansion identity remains
  logical-key based so a coalesced replacement preserves disclosure state without becoming a message
  anchor.

Acceptance criteria:

- Unified pages return mixed message/activity entries in reducer order across page boundaries and
  reproduce the same entries/cursors after restart and full rebuild, independent of arrival order.
- Provider/session/mailbox collisions remain isolated; HQ thread conversations contain no activity;
  progress remains bounded to 200 projected rows and activity-list reads remain capped at 1,000.
- Legacy `ListConversationHistory`, its RPC response, and the HQ client method return the same
  message-only data/order/cursors as before. The new RPC/client method round-trips every canonical
  activity field and complete typed message semantics.
- The TUI renders the unified order without consulting body/`Details`, preserves card disclosure and
  logical-message anchoring across reload/resize/coalescing, and never selects activity for reply,
  archive, draft, final-answer, inbox, unread, or delivery behavior.
- Existing message-only TUI/store behavior tests remain unchanged or gain explicit invariance
  assertions; focused store/RPC/client/TUI race tests, `go test ./...`, `go vet ./...`, and
  `git diff --check` pass.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## 2026-08-25 — Canonical activity synchronization conformance and documentation

Extended the real account-fanout fixture into an interleaved typed message/activity/message conversation, delivered normal encrypted outbox jobs in reverse order, suppressed duplicate wrapper notifications, and proved canonical IDs, typed semantics, activity source fields, mixed reducer order, and legacy message history converge after rebuild and restart. The comparison explicitly preserves device-local recipient/address presentation and delivery state as local facts. Added a validly signed post-revocation activity wrapper test that reaches inbound decryption and causal authorization, is rejected and quarantined, increments revoked-device diagnostics, and changes no canonical, activity, mixed-history, summary, inbox, or delivery projection; public, peer, and malformed account scopes also fail validation. Rewrote README and the event, harness, and design docs around schema 32, dual-stream conversations, account audience/privacy, durable versus replaceable persistence, tail coalescing/backpressure, deterministic ordering, partial-write reconciliation, shutdown draining, canonical versus projected retention, legacy unsigned-row loss, and message-only TUI actions. Full tests, vet, diff checks, focused consistency searches, and store/event/bridge/RPC/client/TUI race suites pass.

### Original plan entry

## Canonical harness activity synchronization, documentation, and conformance

Close the canonical harness-activity integration with transport-level convergence and privacy
conformance, then replace documentation that still describes activity as installation-local and
best-effort. Prove that two active human-account devices reconstruct the same projected mixed
conversation through the ordinary outbox, encrypted gift-wrap, and inbound reducer paths; prove a
revoked source fails closed at that same boundary. Document the dual-stream model, canonical versus
projected retention, audience/privacy rules, queue durability and coalescing, ordering, shutdown,
migration loss of unsigned legacy rows, legacy compatibility, and TUI behavior in the event,
harness, design, and top-level README surfaces.

Scope:

- Strengthen the existing account-fanout store fixture from a single activity-row assertion to a
  real mixed conversation. Author typed messages and activity in one provider/session namespace,
  prepare normal per-device outbox wrappers, receive them through `ReceiveGiftWrap`, and compare the
  complete `ConversationEntry` projection on both devices, including event IDs, display order,
  typed message semantics, every activity source/correlation field, and message-only legacy reads.
- Exercise duplicate/reordered wrapper delivery and projection rebuild in the transport fixture.
  Both devices must converge on the same entry sequence without receipt-time or wrapper-order
  influence, and duplicate wrappers/logical events must not create another message, activity, inbox
  row, or change notification.
- Add activity-specific revoked-device ingress coverage. Construct a correctly signed schema-2
  account activity from a device after its signed revocation, encrypt it for the active creator,
  pass it through the real gift-wrap receiver, and require rejection/quarantine plus unchanged
  activity, conversation, inbox/open/unread, and canonical projected state. Retain event-level tests
  for unrelated-account, peer-addressed, and public activity attempts.
- Preserve the existing canonical authoring boundary: current harness activity uses the active human
  account audience and membership frontier, creates per-active-device outbox rows, and has no public
  write RPC. Protocol-level installation-private activity remains valid only for a genuinely local
  event; public and peer-addressed activity remain invalid.
- Keep all behavior-invariance properties explicit. Activity must not change conversation summaries,
  inbox/open/unread counts, delivery facts, final-answer choice, reply/archive/draft targets, project
  message behavior, or logical-message scroll anchors. Legacy `conversation/history` and
  `activity/list` remain message-only/projected compatibility reads with their existing shapes and
  caps; `conversation/entries` is the typed mixed read.
- Correct `docs/events.md`: schema 2 also defines `harness.activity`; specify its strict neutral
  payload, source mailbox address, allowed scopes/audience, membership parents, signed-wire bound,
  reducer order/coalescing, unsupported-event retention, disposable 200-progress projection,
  1,000-row legacy activity cap, mixed-history pagination, and schema-30 unsigned-row migration loss.
- Correct `docs/harnesses.md`: replace the old local/drop-on-full queue description with the bounded
  serialized buffer, durable versus replaceable classes, replace-at-tail semantics, cancellation-
  aware backpressure, relay-owned persistence context, output-before-activity work ordering,
  deterministic preassigned timeline, reconciliation after partial writes, and orderly shutdown
  draining. Describe canonical activity synchronization and current TUI cards accurately.
- Correct `docs/design.md`: update schema and projection descriptions through version 32, add the
  dual-stream conversation read model, canonical/activity outbox fanout and authorization boundary,
  canonical-log versus disposable-projection retention, rebuild behavior, and the distinction
  between activity source identity and provider-opaque correlation.
- Correct `README.md`: describe inline synchronized activity cards and `e` disclosure without
  implying transcript synthesis; state that inbox/actions remain message-only; update schema 32,
  canonical/outbox/rebuild, privacy, and shutdown language while keeping the user-facing overview
  compact and linking detailed protocol/runtime docs.

Implementation plan:

- Refactor the account-fanout test setup only enough to exchange an interleaved message/activity/
  message timeline through real relay jobs. Add deterministic helpers for delivering selected jobs
  in reverse order and comparing normalized entry pages while preserving exact typed values.
- Add a revoked-device activity test beside existing human-device and activity transport coverage,
  using the real signer and wire codec rather than direct reducer insertion. Assert quarantine and
  `NetworkStatus.RevokedDeviceTraffic` where the receiver classifies the source as revoked.
- Audit and extend behavior-invariance assertions around the transport fixtures; reuse existing
  reducer, store, bridge, RPC/client, TUI, migration, retention, and wire-limit tests instead of
  duplicating lower-level cases already proven.
- Rewrite the stale documentation sections with one vocabulary: canonical event log, projected
  conversation entry, message stream, activity stream, durable work, replaceable snapshot,
  provider/session namespace, source mailbox address, human-account audience, and logical-message
  action/anchor.
- Run targeted shuffled/duplicate/rebuild, account authorization, unsupported schema, signed-wire,
  bridge overload/shutdown, legacy history/RPC, unified history, TUI action/anchor, and migration
  suites before the repository-wide verification and race matrix.

Risks and decisions:

- Absolute reducer display indexes can include other canonical conversation events. Convergence is
  judged by the exact shared entry projection produced from the exchanged canonical set; fixtures
  must deliver all prerequisite membership and conversation events before comparing devices.
- Wrapper receipt order may be non-topological. Missing-parent events are retained and a later
  canonical append rebuilds them; the test must compare the final state, not require each reversed
  intermediate delivery to project immediately.
- A revoked device can still cryptographically create and encrypt bytes. Security depends on local
  audience routing plus causal membership authorization during inbound reduction, so the test must
  reach `ReceiveGiftWrap` and inspect the fail-closed result rather than stop at validation.
- Canonical retention and projected retention intentionally differ. Superseded snapshots and old
  progress events remain signed canonical history while the disposable activity table and unified
  TUI history expose only coalesced winners and the newest 200 progress entries.
- Documentation must describe the implemented current writer: account-addressed activity fans out
  to active human devices. Installation-private is a valid protocol scope, not a promise that the
  current bridge silently downgrades account conversation telemetry to local-only state.

Acceptance criteria:

- Two active account devices receive typed messages and every activity kind needed by the fixture
  through ordinary outbox/gift-wrap/inbound handling and return identical mixed conversation entry
  order and semantics after duplicate/reordered delivery, restart, and full projection rebuild.
- A post-revocation schema-2 activity wrapper is rejected and quarantined as revoked account
  traffic, increments the revoked-traffic diagnostic, and changes no activity, conversation entry,
  summary, inbox/open/unread, or delivery projection.
- Existing unrelated-account and revoked reducer tests, peer/public validation tests, old-schema
  unsupported-byte retention, signed-wire truncation, migration discard, coalescing/reorder,
  progress/query caps, and message behavior-invariance tests remain green.
- `docs/events.md`, `docs/harnesses.md`, `docs/design.md`, and `README.md` agree that activity is a
  canonical synchronized non-message stream with projected retention, exact privacy/audience rules,
  bounded lossless persistence classes, deterministic order, drain semantics, and message-only TUI
  actions.
- Focused store/event/bridge/RPC/client/TUI tests and race runs pass, followed by `go test ./...`,
  `go vet ./...`, `git diff --check`, and documentation consistency searches with no stale local-only,
  drop-on-full, schema-30-current, 64-KiB activity-body, or timestamp-sorted timeline claims.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.


## Completed: Durable harness activity conversation stream — final integration audit

Completed the typed schema-2 message foundation and converted harness activity into a signed,
authorized canonical event stream with deterministic reduction, bounded lossless bridge
persistence, account-device synchronization, projection rebuilds, and a unified non-actionable
conversation history. Preserved legacy message-only behavior and unsupported-event retention,
updated the TUI and protocol documentation, and closed the final audit with exact 1,000-row read
cap coverage plus full-field fake-provider rebuild/restart equivalence. All focused and
repository-wide tests, race suites, vet, build, static boundary searches, and diff checks pass.

## Durable harness activity conversation stream — final integration audit

Harness activity is currently an installation-local, best-effort SQLite projection. The harness bridge can permanently drop activity when its bounded persistence queue fills, and other devices cannot reconstruct the missing timeline. Convert activity into a typed signed canonical event stream that shares transport, persistence, authorization, replay, and deterministic ordering with messages while remaining a distinct, non-actionable entry type.

The user-facing model is one provider-namespaced harness-session conversation containing two semantic streams:

- messages, which retain inbox, unread, reply, archive, delivery, and final-answer behavior;
- harness activity, which appears inline in conversation history but never creates an inbox row, unread count, reply/archive target, draft target, or message delivery claim.

Implement the following:

- Build on the typed message semantics and schema-2 compatibility framework established above. Reuse the same harness-neutral provider/session/operation/item correlation representation; neither activity nor conversation code may reconstruct correlation or presentation from message body or `Details`.
- Add a canonical `harness.activity` event type and typed payload in `internal/event`. The payload must contain only harness-neutral fields: provider, session, operation, optional item ID, activity kind, status, bounded title/body, truncation state, occurrence time, runtime-lifetime identity, and the harness event sequence. Continue supporting operation status, plan, diff, completed command, completed file change, completed tool call, and progress. Do not place Codex method names, JSON-RPC shapes, raw provider payloads, or message technical metadata in canonical activity types.
- Give every projected entry a stable canonical event ID and associate it with the originating full mailbox address plus provider-namespaced session identity. Use operation and item IDs only as provider-opaque correlation fields. A provider/session collision across two providers must not merge conversations or activity.
- Validate kind-specific requirements, statuses, identities, scopes, UTF-8, and bounds in `internal/event/validate.go`. Enforce the actual 64 KiB signed-wire limit, including JSON escaping and envelope overhead; adjust activity body limits or truncate dynamically rather than assuming the existing 64 KiB local body limit fits.
- Authorize activity as private conversation telemetry. It may be installation-private for a genuinely local-only conversation or account-addressed to the same human account/audience as the associated agent or project conversation. It must never be public or automatically peer-addressed. Account-addressed activity must use normal membership parents and encrypted per-device outbox fanout. Reject unauthorized activity from revoked or unrelated installations.
- Replace direct projection writes from `internal/harnessbridge/events.go` with a store operation that signs and appends canonical activity. Remove or narrow the public `activity/upsert` RPC so callers cannot bypass canonical validation; retain a read API for conversation history as needed.
- Keep the bridge persistence path bounded without silent loss:
  - terminal operation states and completed command/file/tool records are durable and apply cancellation-aware backpressure;
  - plan, diff, running-state, and progress snapshots may replace an older pending value with the same logical key before it is signed;
  - when a bounded coalescing buffer has no matching key to replace, apply backpressure instead of dropping a new key;
  - token deltas, spinners, raw reasoning, raw model responses, and other transient provider noise remain ephemeral and need not become canonical events;
  - orderly shutdown drains accepted durable work and the latest accepted coalesced value before teardown.
- Preserve canonical output ordering. Activity and canonical assistant output normalized from the same harness event must enter one serialized persistence path, and their relative timeline order must be deterministic after restart and on every device.
- Extend reduction so arrival order, duplicate delivery, and replay cannot change the result. Derive `harness_activities` entirely from projected canonical events, clearing and rebuilding it during a canonical projection rebuild. Choose coalesced winners using canonical causal/display order and stable source sequence, never SQLite receipt order or the receiving node's clock.
- Preserve the existing logical projection rules:
  - operation, plan, and diff are latest-wins snapshots per conversation/operation;
  - repeated item/progress keys coalesce deterministically;
  - completed command/file/tool records and terminal operation states remain durable history;
  - retain only the most recent 200 projected progress records per provider session and cap activity queries at 1,000 chronological entries;
  - retain canonical events under the existing canonical-log policy even when older entries fall out of the disposable projection.
- Do not manufacture signed history from legacy unsigned `harness_activities` rows. The schema migration may discard those best-effort rows and rebuild from canonical activity events; document that compatibility choice.
- Add or extend a typed read-side union such as `ConversationEntry = MessageEntry | HarnessActivityEntry`. `MessageEntry` must carry typed message semantics and technical sections unchanged; the union must not derive them from text. Conversation history should return both kinds with a stable order derived from canonical reduction. Inbox/conversation summaries must continue to be calculated exclusively from messages. Preserve legacy message-only clients and have older binaries retain the new event bytes as unsupported canonical events.
- Update the TUI to consume the unified ordered history while preserving its existing collapsed/expanded activity cards, failed/truncated disclosure, logical-message scroll anchoring, drafts, final-answer presentation, and message-only reply/archive targeting.
- Update `docs/events.md`, `docs/harnesses.md`, and `docs/design.md` to describe the dual-stream conversation model, synchronization audience, durability classes, ordering, projection retention, privacy boundaries, and the fact that canonical history and projected retention are separate concerns.

Expected implementation areas include:

- `internal/event/{event.go,validate.go,reducer.go}` and their tests;
- `internal/domain/harness_activity.go`, conversation-history types, and change topics;
- `internal/store/{sqlite.go,harness_activity.go,transport.go}` plus schema migration and rebuild tests;
- `internal/harnessbridge/events.go` and overload/shutdown tests;
- `internal/domainrpc`, `internal/hqclient`, and compatibility tests;
- `internal/tui/{activity.go,tui.go}` and timeline tests;
- the canonical-event and harness design documentation.

Completion requires tests proving:

- every activity kind validates, signs, projects, and rebuilds identically;
- shuffled arrival, duplicate delivery, restart, and full rebuild produce byte-for-byte equivalent activity projections and identical conversation order;
- terminal activity is not lost when persistence is blocked past the current 64-entry queue capacity;
- plan/progress bursts coalesce to the latest accepted value without unbounded memory growth;
- two active human-account devices converge on the same inline message/activity timeline through the existing outbox and inbound canonical-event paths;
- revoked, unrelated, peer, and public activity attempts fail closed;
- provider-local session-ID collisions remain isolated;
- provider/session/operation/item association and conversation ordering do not consult message body or `Details`;
- worst-case escaped and multibyte payloads stay valid UTF-8 and below the signed-wire limit with explicit truncation;
- activity changes never alter inbox rows, open/unread counts, final-answer selection, replies, archives, delivery claims, or drafts;
- existing message-only history and legacy unsupported-event behavior remain compatible.

Final audit execution:

- Map every requirement above to its committed implementation and at least one focused test across
  event, store/transport, bridge, RPC/client, TUI, migration, and documentation. Treat preceding
  phase commits as evidence, but rerun the behavior at the current stack tip.
- Close the remaining quantitative read-bound gap with an efficient canonical batch fixture that
  projects more than 1,000 durable activity records in one append and proves `activity/list` returns
  exactly the newest 1,000 in canonical chronological order even when a larger limit is requested.
- Strengthen the fake-provider all-kind integration fixture to compare every projected activity
  field before and after an explicit full projection rebuild and daemon-owned bridge restart, not
  merely the row count. Preserve the later new-runtime replay assertion separately.
- Re-run static boundary searches: no bridge direct SQL activity writes, no public activity write
  RPC, no conversation/TUI correlation reconstruction from message body or `Details`, no stale
  local-only/drop-on-full/schema-30-current documentation, and no provider-specific vocabulary in
  canonical activity types.
- Execute focused normal and race suites for event, store/transport, bridge, RPC/client, and TUI;
  then run repository-wide tests, vet, diff checks, and the project build. Inspect the final stacked
  diff and commit history so the audit does not accidentally absorb ignored PLAN/COMPLETED files.

Audit decisions:

- Cross-device equality means canonical event IDs, reducer order, typed message semantics, and full
  activity projection. Message recipient installation presentation, resolved address labels/kinds,
  and delivery state are device-local facts and are normalized only in convergence assertions.
- The 1,000-row test uses one batch of valid signed canonical events and one reducer rebuild; issuing
  1,001 public authoring calls would test quadratic fixture cost rather than the read contract.
- No production rewrite is required when a clause already has direct code and test evidence. Any
  newly observed mismatch is fixed in scope before the umbrella can be archived.

Final audit acceptance:

- The all-kind fake-provider projection is byte-for-byte stable across explicit rebuild and restart,
  and the canonical legacy activity read enforces its exact 1,000 newest-row cap.
- Every original completion bullet has current code/test evidence; static searches find no bypass or
  stale semantic claim, and all focused/full normal, race, vet, build, and diff checks pass.
- `PLAN.md` contains no remaining task after this entry is archived, and the active goal is marked
  complete only after the final commit and ignored completion record are verified.

Run `go test ./...`, relevant race tests for the bridge/store/TUI, `go vet ./...`, and `git diff --check`.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## Mouse-wheel scrolling by hovered TUI pane — completed

Implemented coordinate-routed three-row mouse-wheel scrolling for the inbox and message panes, shared cell-motion view configuration, inert modal and non-scrollable regions, preserved focus and composer bindings, comprehensive regression coverage, and updated operator documentation.

## Mouse-wheel scrolling by hovered TUI pane

Add native vertical mouse-wheel navigation to the HQ TUI. Route each wheel event by the pointer's terminal coordinates so the hovered inbox or message pane scrolls without changing keyboard focus, entering compose mode, or disturbing an active draft/reply binding.

- Add failing tests in `internal/tui/tui_test.go` before implementation. Cover:
  - normal views request `tea.MouseModeCellMotion`;
  - wheel up/down over the inbox moves its selection by three rows, clamps at both ends, and leaves `paneFocus` unchanged;
  - inbox selection changes use the existing message-viewport reset and context/history-loading behavior when not composing;
  - wheel up/down over the message pane moves by three rendered lines through `scrollMessagePane`, remains bounded, marks a real movement as manual, and retains the existing logical message anchor behavior;
  - exact pane boundaries route correctly using the zero-based `Y` coordinate from `responsivePaneLayout`;
  - reply-pane, help-row, out-of-bounds, horizontal-wheel, blocking-connection, recipient-picker, project-setup, and agent-manager events are no-ops;
  - scrolling a pane under the pointer never changes focus or the active composer's answer/draft association.
- In `internal/tui/tui.go`, encapsulate mouse-wheel routing in a focused helper instead of expanding the main update loop with duplicated navigation logic. Use a named three-line wheel-step constant, reject unsupported directions and coordinates, and reuse the existing inbox-selection/context and message-scroll paths so mouse and keyboard behavior cannot drift.
- Set `tea.View.MouseMode` to `tea.MouseModeCellMotion` for every TUI view state. Keep alternate-screen configuration DRY when applying the shared view settings.
- Do not add independent inbox offset state, click handling, hover focus, horizontal scrolling, modal-list scrolling, or reply-textarea wheel scrolling in this task.
- Update the README's TUI controls and scrolling description to explain pane-under-pointer wheel behavior and note that terminal-native text selection may require Shift while mouse reporting is enabled.
- Run `go test ./internal/tui` and `go test ./...`.

Acceptance criteria: vertical wheel input scrolls the hovered inbox or message pane by three rows with correct boundary clamping and existing anchor semantics; keyboard focus and compose state never change because of scrolling; unsupported locations, directions, and modal states are inert; no new dependency is introduced; and all tests pass.

Implementation plan:

- Modify `internal/tui/tui_test.go` first with table-driven and focused behavioral tests for the
  shared view settings, exact zero-based pane boundaries, inbox and message movement/clamping,
  unchanged focus and compose binding, context command behavior, and all required no-op states.
- Modify `internal/tui/tui.go` to add a named three-row wheel step, one focused mouse-wheel router,
  and one DRY view constructor that applies alternate-screen and cell-motion mouse settings to
  normal and agent-manager views. Reuse `resetMessageViewport`, `withContextCommand`, and
  `scrollMessagePane`; do not introduce separate offsets or modal mouse behavior.
- Modify `README.md` to document hovered-pane wheel scrolling and the Shift modifier commonly
  needed for terminal-native selection while mouse reporting is enabled.
- Run the new focused tests first, then `go test ./internal/tui`, `go test ./...`,
  `go test -race ./internal/tui`, `go vet ./...`, `go build ./...`, and `git diff --check`.

Risks and decisions:

- Bubble Tea mouse coordinates are zero-based, while the rendered panes are vertically stacked.
  Route inbox rows `[0,inboxHeight)`, message rows
  `[inboxHeight,inboxHeight+messageHeight)`, and treat reply/help/out-of-bounds rows as inert.
- Bubble Tea sends mouse events through the normal update path after the renderer callback. Handle
  `tea.MouseWheelMsg` directly and do not install an `OnMouse` loopback callback.
- While composing, inbox-wheel selection may move but must not reset/rebind the active detail or
  draft; only non-compose inbox changes run the existing viewport/context-loading path.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.


## 2026-08-26 — Lawful causal reduction algebra and pure reducer seams

Implemented a standard-library semilattice/set/causal-frontier core with law tests, introduced SQL-free event-state contracts and architecture guards, split canonical wire validation/signing from pure projection reduction, and made the existing `event.Reduce` API a compatibility facade. The full suite, focused race tests, vet, build, and diff checks passed; commit `9080736` records the implementation.

### Original plan entry

## Lawful causal reduction algebra and pure reducer seams

Establish the functional core that every later schema-3 and incremental projection change will use.
This task changes internal structure without changing the current canonical wire or SQLite write
behavior, so it remains independently reviewable and the existing system stays green.

### Algebra and typed causal model

- Add `internal/reduction` with an immutable/copy-on-write `Set[K comparable]`, an explicit
  `JoinSemilattice[T]` dictionary (`Empty`, `Join`, `Equal`), generic folds, causal relation
  helpers, and deterministic frontier/maxima operations. Use only the standard library; do not add
  a reflection registry or external functional-programming dependency.
- Add branded event/resource identifiers at the reduction boundary so event IDs, aggregate keys,
  and projection keys cannot be accidentally interchanged as untyped strings.
- Document the algebraic laws beside the production abstractions and provide reusable test helpers
  for identity, associativity, commutativity, idempotence, duplicate tolerance, and
  chunk/permutation invariance.

### Pure reducer decomposition

- Introduce `internal/eventstate` as the SQL-free domain reduction package. Define a read-only
  causal query interface, immutable decoded fact inputs, layered validity/readiness/authorization
  results, projection support/provenance, and typed projection deltas.
- Extract concrete pure reducers from the monolithic mutable `event.State` implementation for
  mailbox/installation state, agents/sessions, peers/shares, human accounts/devices,
  messages/message state/threads, and harness activity. A reducer may not perform SQL, signing, RPC,
  transport, logging, clock reads, or mutate caller-owned input.
- Keep the current `event.Reduce` API as a compatibility facade over a full-set batch composition
  of those concrete reducers. Preserve current schema-1/2 behavior and exact observable projections
  in this task; protocol semantics change in the next queued task.
- Reuse `projectstate.Apply` as the pure project transition function and define the adapter seam by
  which authoritative and replica project facts will join the common reducer contract later.

### Tests and acceptance

- Write failing algebra law tests first, followed by generated/shuffled DAG tests for causal
  frontiers, missing parents, duplicates, and concurrent maxima.
- Port existing reducer characterization tests to assert the facade and extracted reducers return
  identical records, messages, threads, activities, agents, accounts, peers, shares, and ordering.
- Add an architecture test that rejects imports from SQL/store, RPC, TUI, signing, or transport
  packages into `internal/reduction` and `internal/eventstate`.
- Run `go test ./...`, `go test -race ./internal/reduction ./internal/eventstate ./internal/event`,
  `go vet ./...`, `go build ./...`, and `git diff --check`.

### Files

- Create `internal/reduction` and `internal/eventstate` production and test files.
- Refactor `internal/event/reducer.go` and its tests into the facade over the new core.
- Touch `internal/projectstate` only where a narrow adapter or immutability fix is required.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.


## 2026-08-26 — Schema-3 causal authority and clean protocol break

Made canonical schema 3 the sole accepted format, removed legacy schema decoding and the SQLite
migration ladder, bumped domain wire/SQLite to 7/33, and made old non-empty databases fail with
reinitialization guidance. Replaced trust/share authorization with local peer bindings and
directional mailbox grant/revoke/observation capabilities, added explicit account authorities,
receiver observation frontiers, remove-wins conflict behavior, full relay/pairing/bootstrap
coverage, and updated protocol documentation. The recovered 84-message TUI Work transcript is
stored in `TUI-Work-thread.md`. Vet, the full test suite, build, and diff checks passed; commit
`316dd76` records the implementation.

### Original plan entry

## Schema-3 causal authority and clean protocol break

- Replace exported raw type/payload pairings with validated schema-3 typed facts and constructors.
  Schema 3 is the only emitted or accepted canonical schema; remove schema-1/2 decoding,
  `legacy_message.go`, legacy detail parsing, and compatibility tests.
- Add explicit typed authority references that must be a validated subset of causal parents. Derive
  aggregate/resource keys from typed payloads rather than duplicated strings or presentation text.
- Replace global peer trust/share authorization with peer-delivered mailbox capabilities:
  `mailbox.access.grant` authorizes one peer installation/signer for one target mailbox;
  `mailbox.access.revoke` descends from the grant/frontier; peer-addressed actions reference the
  matching grant. Traffic before revocation stays authorized, traffic after or concurrent with a
  revoke fails closed, and a later causally descended grant restores access.
- Separate cryptographic validity, causal readiness, historical authorization, and current local
  routing/block policy. Blocking a peer stops new transport without hiding earlier authorized
  history. Convert account membership to the same explicit authority-reference mechanism.
- Codify resource conflict rules: remove-wins revocation/rejection/retirement, archive over
  concurrent restore, causally later restore reopening, retained answer sets, independent
  cancellation, deterministic activity winners, and explicit unique-root conflicts.
- Set canonical schema to 3, local domain wire to 7 where the event/domain DTOs change, and SQLite
  schema to 33. Delete the 7→32 migration ladder. A non-empty older database fails startup; there is
  no migration/export/translation path. The operator manually archives/removes old databases and
  reinitializes/re-pairs. Keep the provisional Nostr kind and lifecycle wire version unchanged.
- Update protocol/design/Nostr documentation and validate fresh bootstrap, peer capability exchange,
  account pairing, relay round trips, old-version rejection, and all conflict cases.

### Implementation map and test order

- Start with schema-3 fixture and validation tests in the wire package, then causal authorization
  reducer tests, then store/transport integration tests. Only after they fail for the intended
  reasons should writers and database bootstrap change.
- Change the canonical types/validation in `internal/eventwire`, authority and conflict semantics in
  `internal/eventstate`, and the capability authoring/routing paths in `internal/store`. Update local
  DTOs only where schema-3 types cross RPC; keep the lifecycle protocol unchanged.
- Replace the SQLite schema definition and configuration path in one change after protocol tests are
  green. Preserve only fresh schema-33 creation and same-version reopen; delete migration SQL and
  legacy fixtures rather than adapting them.
- Risks to test explicitly: reciprocal peer access is directional, a revoke must still be deliverable
  before local blocking disables routing, missing capability parents remain unresolved rather than
  unauthorized, and account authority must not be inferred from arbitrary causal parents.

## 2026-08-26 — Incremental causal index and core projections

Replaced the projection checkpoint with reducer-versioned metadata and durable causal, authority,
waiter, resource, frontier, support, and layered reduction indexes. Normal ingestion now inserts and
indexes exact facts transactionally, computes a directional fixed-point impact closure plus read-only
support ancestors, runs the shared pure reducers only over that closure, and upserts affected core
projections without clearing them or scanning the canonical log. Late parents wake descendants;
account and installation support remains directional so unrelated messages are not rewritten. The
batch reducer and projection clears remain confined to explicit/startup repair. Frontier and account
authoring queries now use indexed resource closures. Added startup-version repair, atomic fault,
late-parent, no-clear, current-index, and unrelated-generation regression tests. Project replay was
also made project-scoped to preserve unrelated projects and unsigned operation state. Vet, all tests,
pure-core race tests, build, and diff checks passed.

### Original plan entry

## Incremental causal index and core projections

- Replace `projection_checkpoint(event_count, rebuilt_at)` with reducer-versioned metadata and
  durable indexes for forward/reverse causal edges (including missing parents), unresolved waiters,
  event-to-resource membership, authority dependencies, aggregate frontiers, layered status/reason,
  and projection support generations.
- Implement one transactional fixed-point ingestion loop: insert exact signed facts and indexes,
  seed new facts/waiters/resource peers/reverse dependents, reclassify only that closure, run the
  affected pure reducers, diff prior values, patch changed projection rows, reconcile affected
  outbox/change topics/checkpoint, then commit before notification.
- Make installation/mailbox/agent/session, capability, account/device, message/message-state/thread,
  and activity projections incremental. A late parent resolves waiting descendants; concurrent
  revocation retracts losing projections; regrant/restore reprojects only supported descendants.
- Remove canonical full scans from normal frontier, authority, parent, message-state, and agent
  operations. Normal local and remote writes must not call the batch reducer, clear projection
  tables, update every event status, or rewrite unsigned leases/receipts/attempts/runtime state.
- Retain an explicit offline repair path that clears only rebuildable indexes/projections and folds
  all schema-3 facts through the batch oracle. Startup runs repair only for missing/mismatched
  reducer metadata.
- Add transaction fault tests at every ingestion boundary plus SQL-trace tests proving an unrelated
  append performs no all-event query, projection-table clear, or unrelated-row write.

## 2026-08-26 — Incremental projects, outbox, and conversation ordering

Added typed project, resource, agent, and accepted-message keys to causal indexing so related
projects enter the same affected closure while unrelated projects remain untouched. Authoritative
and replica replay, project-input discovery, legacy projection support, cleanup, and command-derived
state are now project-scoped; unsigned runtime/worktree/retirement data and unrelated projects are
preserved. Outbox reconciliation remains limited to the routing closure and retains exact bytes and
attempt state. Removed persisted global `display_order` from messages and activities. Conversation
pages now derive deterministic parent-before-child order from causal edges, with immutable time/ID
tie-breakers, conversation-local positions, and strict event-anchored cursors. Standalone activity
ordering/retention uses immutable occurrence time and event ID. Added causal clock-skew ordering and
schema regressions; the full cross-package suite, vet, build, and diff checks passed.

### Original plan entry

## Incremental projects, outbox, and conversation ordering

- Route project events through typed project/resource/agent aggregate keys. Incrementally replay only
  the affected project chain through `projectstate.Apply`; preserve the last unambiguous head on a
  fork and re-evaluate global agent/resource exclusivity through their own aggregate keys.
- Incrementally maintain authoritative/replica projects, project inputs, acceptances, dispatch
  records, output provenance, queued work, resource claims/health, and command results without
  clearing project tables or disturbing unsigned runtime/worktree/retirement operations.
- Reconcile only outbox rows affected by a new fact or capability/account routing frontier. Preserve
  exact bytes and existing relay-attempt state for unaffected rows.
- Remove global dense `display_order`. Persist immutable conversation sort components and derive a
  deterministic parent-before-child order within a requested conversation. Update local wire-7
  entry DTOs and cursors to conversation-local positions; ordering is presentation-only and cannot
  affect authorization or winner selection.
- Add differential tests for project forks/repair, remote replicas, cross-project exclusivity,
  project input/output, outbox fanout/revocation, late conversation entries, activity retention,
  and paginated mixed message/activity history.

## 2026-08-26 — Differential incremental-reduction conformance

Added a reusable differential oracle that snapshots the reducer-owned SQLite boundary and fully
paged conversation APIs, performs an offline batch rebuild over the identical canonical log, and
reports the first divergent normalized row. Added deterministic signed-DAG schedules covering
prefixes, reverse and late dependencies, seeded shuffles, duplicates, message lifecycle state,
capability revoke/regrant, human-device membership, activity coalescing, project forks, and global
resource/agent conflicts. The harness exposed a real incremental divergence: a later revocation
reclassified a previously projected message but left its typed row visible. Incremental projection
now retracts impacted messages, threads, and activities whose canonical facts cease to project.
Build, vet, the full test suite, differential tests, race tests, and diff checks pass.

### Original plan entry

## Differential incremental-reduction conformance

- Build a reusable differential harness that feeds generated DAG prefixes, shuffled permutations,
  duplicates, and missing dependencies through incremental ingestion and compares event status,
  frontiers, projections, outbox, projects, and conversation pages with a clean batch rebuild.
- Cover late capabilities, revoke/action concurrency, regrant, archive/restore/reject, account
  membership, activity coalescing, project forks, and cross-project resource/agent conflicts.
- Require incremental and batch results to converge for every supported arrival order and duplicate,
  with diagnostics that identify the first divergent event, resource, or projection row.

Implementation plan:

- Add `internal/store/reduction_conformance_test.go` with a reusable test-only harness. It will run
  deterministic prefix, reverse, seeded-shuffle, duplicate, and late-dependency schedules against
  fresh SQLite fixtures. At every requested checkpoint it will snapshot incremental state, invoke
  the existing offline `Rebuild` oracle over the same canonical log, snapshot again, and report the
  first differing table, API page, or normalized row.
- Make the snapshot contract explicit. Compare canonical reduction status/reason, layered event
  status, causal/authority/waiter/resource indexes, generation-free frontiers and projection
  support, core mailbox/agent/message/thread/account/access projections, stable outbox routing and
  state, authoritative and replica project projections, harness activities, and fully paged
  conversation summaries/entries. Exclude generation counters, repair timestamps, mutation/change
  receipts, delivery leases, relay attempts, runtime attempts, resource-health observations, and
  unsigned local drafts because batch reduction does not own them.
- Add `internal/store/reduction_conformance_scenarios_test.go` with small deterministic signed DAG
  builders and scenario tests for late parents, duplicates, archive/restore/reject, capability
  arrival/revoke/action/regrant, human-device membership, activity coalescing, project forks, and
  cross-project resource/agent conflicts. Reuse store signing, project payload, and account helpers
  rather than duplicating production reduction logic.
- If the harness finds a real divergence, make the narrowest production correction in the owning
  reducer/projection file—principally `internal/store/causal_index.go`, `internal/store/sqlite.go`,
  or `internal/store/project_projection.go`—and retain the failing schedule as a regression test.

Test strategy:

- Write the normalized snapshot and one late-parent/duplicate scenario first, prove that deliberate
  projection corruption produces a useful first-row diagnostic, then add each causal domain.
- Use fixed timestamps, UUIDs, secrets, and PRNG seeds so failures reproduce exactly; bound the
  schedule set rather than enumerating factorial permutations.
- Run the focused conformance suite repeatedly and under the race detector, followed by build, vet,
  and the full repository suite.

Risks and decisions:

- A same-database before/after oracle deliberately compares reducer-owned results, not transport or
  workflow side effects that `Rebuild` preserves. The table allowlist makes that boundary reviewable.
- Project input reconciliation may append deterministic acceptance facts while ingesting an input.
  The harness snapshots the resulting canonical log before rebuilding, so the oracle receives the
  exact same facts and tests projection convergence rather than re-running external workflow intent.
- Conversation cursors and display order are compared through public paged APIs as well as backing
  rows, catching ordering bugs that a table-only snapshot would miss.
- The scenario suite will stay small enough for normal CI; large-history cost belongs in the next
  bounded-work and benchmark phase.

## 2026-08-26 — Bounded-work, restart, and protocol conformance gates

Added structural closure-size, unchanged-generation, indexed-query-plan, and large-history benchmark
gates plus a schema-3/database-33/domain-wire-7 reopen and repair matrix. Ordinary ingestion now
advances metadata without recounting history, preserves only affected mailbox/agent operational
state, prunes activity by affected partition, scopes observation lookup to relevant grants, skips
irrelevant project-input/command scans, and resumes pending commands at relay ingress and node
startup. Human account authoring now reads the reducer-maintained active creation/acceptance
authority projection instead of re-verifying every causally descended account message. On an Apple
M2 Pro with 10 fixed iterations, independent append improved from 12.30 ms to 2.84 ms at 32 history
entries and from 140.89 ms to 4.85 ms at 512; affected work stayed at 4 rows and post-change
allocations stayed essentially flat (~160 KB and ~2.2k per operation). Exact canonical/outbox bytes,
unsigned drafts, receipts, and relay state survive reopen and repair. Build, vet, the full suite,
focused protocol tests, store/node race suites, and diff checks pass.

### Original plan entry

## Bounded-work, restart, and protocol conformance gates

- Add large-history benchmarks and regression assertions that normal work is bounded by the affected
  closure rather than total history. Record useful baseline and post-change measurements.
- Add crash/reopen conformance, fresh bootstrap, relay, mutation retry, subscriptions, project
  command processing, repair, and durable-draft restart coverage on schema 3/database 33/domain wire 7.
- Require normal writes never to scale with total history or clear complete projection tables.

Implementation plan:

- Add `internal/store/reduction_performance_test.go` with deterministic independent-history fixtures,
  closure-size assertions using the ingestion transaction's impacted/affected tables, generation
  checks proving unrelated reductions are untouched, query-plan guards for canonical event-type
  lookups, and benchmarks at small and large history sizes. Keep setup outside benchmark timing and
  report affected rows per operation alongside time and allocations.
- Remove history-wide work from ordinary ingestion while keeping repair deliberately whole-log:
  advance projection metadata by the number of newly inserted facts instead of recounting the log;
  preserve mailbox activity, named-agent activity, and ownership only for projections present in
  the affected pure state; prune activity retention only for affected source/session partitions;
  and skip or resource-scope mailbox-observation lookup when a batch has no relevant inbound peer
  traffic.
- Give canonical project-command/status lookups an explicit schema-33 index, installed idempotently
  on same-version reopen, and avoid polling all project commands after unrelated canonical appends.
  Preserve replay after a lost response or restart by processing commands when they arrive and once
  the node has installed its runtime command handler.
- Add a focused protocol restart matrix across store/node integration tests. Assert a fresh database
  emits only schema-3 canonical events under SQLite 33, negotiates domain wire 7, preserves unsigned
  drafts and mutation receipts across reopen, resumes subscriptions after node restart, retains
  exact relay/outbox state, processes a pending project command once, and lets explicit repair
  rebuild projections without erasing operational state.

Test strategy:

- Land the closure and query-plan tests first and run the benchmark before production changes to
  record a baseline. Apply narrow fixes one source of unbounded work at a time, retaining a
  regression assertion for each discovered path, then record the post-change benchmark under the
  same machine/process conditions.
- Reuse existing node, relay, mutation, project-command, and draft fixtures rather than inventing a
  second protocol implementation. Use fixed IDs and bounded histories in normal tests; reserve the
  larger fixtures for benchmarks and one non-timing structural regression.
- Run focused store/node protocol tests repeatedly and under the race detector, then run build, vet,
  the full repository suite, and `git diff --check`.

Risks and decisions:

- Wall-clock ratios are useful measurements but flaky correctness gates. CI assertions therefore
  inspect affected-row counts, unchanged generations, and indexed query plans; benchmark numbers
  are recorded for engineering evidence only.
- Account membership changes legitimately touch the account's delivery closure, and activity
  retention legitimately touches one source/session partition. "Bounded" means proportional to
  that typed affected closure, not universally constant work.
- Same-version schema-33 reopen may add only idempotent local indexes/tables; canonical wire and
  projection semantics remain schema 3 / DB 33 / domain wire 7 with no legacy migration path.
- Repair remains the sole whole-log/whole-projection path and must preserve unsigned drafts,
  receipts, relay attempts, runtime leases, and other operational state outside reducer ownership.

## 2026-08-26 — Legacy reducer cleanup and final pure-core contract

Replaced the reducer's implicit orchestration with one named, effect-free pipeline and one stable
`event.Reduce` facade shared by dependency-closed ingestion and the complete-log repair oracle.
Removed the duplicate affected reducer, unused store reducer interface, unused generic
fact/decision/delta prototype, and duplicate message-order pass. Architecture tests now pin the
stage inventory and forbid alternate reducer entry points or direct state-package imports; facade
equivalence covers shuffled and duplicated schema-3 DAGs. Current documentation now describes
schema 33's causal support indexes, incremental projections, derived conversation order, and the
separate Nostr wrapper/canonical schema versions. Build, vet, the full suite, pure reducer race
tests, and store race tests pass; the relay node integration suite passes normally, while its
four-second asynchronous capability deadline is consistently too short under race instrumentation.

### Original plan entry

## Legacy reducer cleanup and final pure-core contract

- Remove the old monolithic reducer/rebuild write path, obsolete schema/status helpers, stale docs,
  migrations, compatibility fixtures, and transitional APIs. Keep only the batch repair oracle and
  shared pure reducers.
- Require every canonical domain to use the pure-core contract and no old compatibility path to
  remain.

Implementation plan:

- Replace the hard-coded reducer call sequence with one effect-free named pipeline in
  `internal/eventstate`. The pipeline dictionary will group control/causal readiness, peer and
  mailbox capabilities, human account membership, domain authorization, mailbox/agent projection,
  message lifecycle/thread/order, and harness activity stages. Raw wire inspection initializes an
  owned state once; every stage receives only that state and performs no SQL, signing, clocks,
  transport, RPC, or caller-owned mutation.
- Keep one reducer function for both dependency-closed incremental sets and the full batch repair
  oracle. Remove the duplicate `ReduceAffected` facade, the unused store `Reducer`/`ReducerFunc`
  abstraction, the unused generic fact/decision/delta prototype, and the duplicate message-order
  pass. Keep `internal/reduction`'s used immutable set, semilattice, fold, and causal relation laws.
- Make the final boundary mechanically reviewable: `internal/event` is the only wire/state facade;
  normal incremental and explicit repair paths both invoke its single pure reducer after selecting
  their input set; only explicit repair reads the whole canonical log or clears full rebuildable
  projections. Add architecture tests for the pipeline's complete ordered stage inventory, facade
  equivalence, the absence of transitional reducer symbols, and effectful-import exclusions.
- Remove or correct stale reducer/projection documentation. Update `docs/design.md` from the deleted
  checkpoint/schema-32/dense-order/share model to schema 33's causal indexes, reducer metadata,
  incremental projection support, capabilities, account authority, conversation-local order, and
  explicit offline repair oracle. Clarify that the Nostr rumor's schema 1 is the wrapper envelope,
  while its embedded canonical event is schema 3.

Test strategy:

- Add failing contract tests before changing production: assert the expected named stage list once,
  compare the public event facade with the direct pure core over shuffled/duplicated schema-3 DAGs,
  and statically reject `ReduceAffected`, store reducer interfaces, effectful imports, and any
  whole-log reducer call outside the explicit repair function.
- Run the differential incremental-versus-rebuild oracle after consolidation to prove both callers
  still share semantics, plus existing algebra/property and schema-3 authorization suites.
- Finish with pure-core race tests, focused store conformance/race tests, build, vet, the full suite,
  documentation searches for obsolete schema/checkpoint language, and `git diff --check`.

Risks and decisions:

- Go cannot encode Haskell typeclasses directly; the named stage interface and immutable algebra
  dictionaries provide the useful property here—explicit composition and law-testable behavior—
  without reflection, registration magic, or an external functional library.
- The pure pipeline may mutate only its newly allocated internal state. Inputs and projected slices
  remain copied at the boundary, so callers cannot observe mutation and stages remain deterministic.
- `event.Reduce` stays as the stable package facade. "Remove the old reducer" means eliminating
  duplicate/transitional entry points and orchestration, not removing the explicit full-set repair
  oracle or schema-3 rejection tests.
- Historical planning records and the recovered Alice transcript intentionally describe older
  schemas and are not product documentation; only current README/docs and production comments are
  cleanup targets.

## 2026-08-26 — Dismiss the project recipient chooser with pane-navigation keys

Tab and Shift-Tab now dismiss the project/direct-recipient chooser through the same state transition
as Escape, clearing its query and cursor, returning focus to the inbox, and preserving inbox
selection and scroll state. Focused regression coverage exercises both keys and the full TUI suite
passes.

### Original plan entry

- Pressing <tab> or <s-tab> from the [project · choose project work or direct recipient]
  pane should be equivalent to pressing <esc> from there.

## 2026-08-26 — Keep bottom-anchored message panes tailing live content

The TUI now records whether the selected message pane is at its last full viewport before either a
snapshot or history refresh. If so, it repins the refreshed pane to the new last full viewport,
including when a newly arrived message is taller than the pane; non-bottom logical anchors continue
through the existing reconciliation path. Regression coverage exercises both refresh forms, and
the focused, full, vet, repository-wide, and TUI race suites pass.

### Original plan entry

- When the message pane is scrolled to the bottom and a new message arrives, we should tail it
  (scroll with it). In other words, if the scrollbar is at the bottom, we should keep it there.

## 2026-08-27 — Rust behavior ledger and product boundary

Recorded the immutable final Go commit/tree and classified 191 externally meaningful compatibility,
algebra, identity, messaging, relay, harness, project, client, security, operations, and regression
behaviors as retain, redesign, or drop with required/deferred/excluded release disposition and a
downstream owner. Added source and command/TUI coverage indexes, retained all four former Go-plan
regressions, and accepted focused ADRs for Linux/macOS plus single-executable packaging, encrypted
identity backup and Go-state isolation, and first-release client/provider workflows. Added a
portable verifier for baseline identity, source markers, unique/valid classifications, regressions,
ADR acceptance, and unresolved markers; it failed first on absent artifacts and now passes with
Bash syntax and ShellCheck. Updated downstream roadmap packages to carry the platform, packaging,
and backup decisions. The unchanged Go baseline passes build, vet, cached tests, and a fresh full
`go test -count=1 ./...` run.

### Original plan entry

- **[design/high] Establish the Rust behavior ledger and product boundary** — Record the frozen Go
  baseline without changing it, then classify every externally meaningful capability from the
  authoritative sources as **retain**, **redesign**, or **drop** in a tracked Rust-era behavior
  ledger. Resolve the first-release feature/deferred boundary, supported operating-system surface,
  identity backup scope, CLI/TUI workflow inventory, and other product-level choices. For choices
  not fixed by the rewrite design, select a conservative first-principles default and record it in
  a focused ADR. Preserve the four former Go-plan findings as Rust requirements: causal-maximal
  regrant authority, one canonical conversation comparator, indexed pagination, and non-disruptive
  relay wakes. Complete this work when no Go-facing compatibility assumption or retained user
  workflow remains uncategorized and later tasks can rely on a stable product boundary.

  Implementation plan:

  - Create `docs/rust/behavior-ledger.md` as the traceable source of truth for the frozen Go
    baseline, source inventory, compatibility boundaries, retained capabilities, deferred scope,
    and the four inherited regression requirements. Give every behavior a durable capability name,
    a `retain`/`redesign`/`drop` classification, an explicit first-release/deferred/excluded
    disposition, and a downstream specification or work-package owner.
  - Add focused accepted ADRs under `docs/adr/` for the Unix first-release platform and single
    executable packaging boundary; encrypted identity backup with complete Go-state isolation; and
    the supported CLI, Ratatui, and managed-provider workflow boundary. Keep protocol field values,
    provider version selection, and quantitative budgets owned by their later specification tasks.
  - Add `scripts/verify-rust-behavior-ledger.sh` first and demonstrate that it fails while the
    ledger/ADRs are absent. Make it check the frozen commit/tree, unique behavior IDs, allowed
    classification/disposition values, source coverage markers, inherited regression IDs, and ADR
    references so uncategorized additions fail visibly.
  - Verify the frozen Go revision with its existing full test suite, run the ledger verifier, and
    run the repository's normal test/vet/build gates. Review the final ledger directly against
    `rust-rewrite-design.md`, `rust-port.md`, the algebra note, `README.md`, `docs/`, CLI dispatch,
    embedded agent help, and project/harness specifications before archiving this plan entry.

  Risks and decisions:

  - A ledger can appear exhaustive while combining distinct authority or recovery rules. Keep
    security, algebra, transport, runtime, client, and project behaviors in separate rows and use a
    source-coverage index rather than relying on prose claims of completeness.
  - `redesign` means the capability remains desired but its Rust semantics are specified afresh; it
    does not imply Go wire, schema, command, UI, timing, or diagnostic compatibility.
  - The recorded Go baseline is the final pre-roadmap commit and tree, not the current branch that
    contains Rust planning documents. No Go source, fixture, schema, or deployment file is changed.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-27 — Causal fact algebra and semantic fact catalog

Specified an implementation-independent causal algebra with structural and usable reachability,
typed dependencies, deferred reconsideration, exact causal frontiers, projection retraction,
historical authority, explicit conflict registers, deterministic presentation, batch reduction, and
normalized observations. Cataloged 48 canonical and remote-control fact families with complete
authority, validation, conflict, projection, retention, and observation rules. Added 115 named
acceptance scenarios covering all nine algebraic laws, authority races, domain conflicts, project
invariants, remote-control isolation, security attacks, and inherited regressions, plus a portable
completeness verifier. Both Rust-spec verifiers, Bash syntax, ShellCheck, whitespace checks, Go
build/vet, and the fresh full Go test suite pass.

### Original plan entry

- **[design/high] Specify the causal fact algebra and semantic fact catalog** — Create tracked,
  implementation-independent specifications for the add-only fact set, graph terminology,
  reachability, usability, deferred dependencies, causal maxima, explicit historical authority,
  projection retraction, deterministic conflict rules, and canonical presentation order. Catalog
  every retained fact family for identity, installation-local control, peers, mailbox capabilities,
  human accounts, conversations, activity, agents, sessions, projects, and remote control. For each
  fact define required parents and authorities, validation, unresolved behavior, conflict policy,
  projection effects, retention class, and normalized observations. Turn all nine algebraic laws,
  safety properties, and known Go defects into named acceptance scenarios. Complete this work when
  the pure reducer can be implemented without consulting Go control flow or prose with an undefined
  conflict outcome.

  Implementation plan:

  - Add `docs/rust/causal-algebra.md` to define semantic fact identity, the add-only set and merge,
    structural versus usable reachability, dependency roles, decision categories, reconsideration,
    causal frontiers, projection support/retraction, complete-batch reduction, incremental equality,
    explicit historical authority, conflict registers, and the sole presentation comparator.
    Specify normalized reducer output without importing wire, SQL, clock, transport, or runtime
    representation.
  - Add `docs/rust/semantic-fact-catalog.md` with one durable catalog ID for every retained fact or
    signed remote-control family. For each entry record its semantic payload, scope/signer, required
    parents, authority references, validation, unresolved behavior, concurrent-conflict policy,
    projection effects/support, retention, and normalized observations. Expand the tricky peer,
    revoke/regrant, human-membership, conversation/activity, agent/session, global project-claim,
    linear project-history, dispatch, and remote-control rules into implementation-ready sections.
  - Add `docs/rust/acceptance-scenarios.md` defining deterministic fixture vocabulary and normalized
    observations, then name scenarios for all nine laws, graph/dependency safety, every authority
    race, message/activity conflict and ordering, agent/session conflicts, project transitions and
    cross-project invariants, remote commands, projection retraction, and all four inherited
    regressions.
  - Add `scripts/verify-rust-causal-spec.sh` first and show that it fails while the specifications
    are absent. Make it verify catalog field completeness and unique IDs, required fact families,
    nine named laws, required attack/regression scenarios, cross-document links, allowed retention
    and protocol-class values, and the absence of unresolved markers. Extend the behavior-ledger
    verifier only if the new specifications expose a product-boundary omission.
  - Run Bash syntax and ShellCheck on both specification verifiers, both verifiers themselves,
    whitespace checks, the Go build/vet gates, and a fresh full Go test suite to prove the frozen
    scenario source remains intact before archiving this plan entry.

  Risks and decisions:

  - Semantic facts must not freeze JSON fields, Nostr kinds, SQL rows, numeric protocol limits, or
    Go type names. Protocol work will map each catalog entry explicitly later.
  - Every declared parent is a required causal dependency; authority references are typed roles
    within that set. An absent or currently unusable parent blocks semantic support, and an
    unrelated usable parent can never supply authority.
  - Safety-sensitive singleton state uses remove-wins or an explicit multivalue conflict, never a
    timestamp/fact-ID winner. Signed times and stable IDs are reserved for deterministic
    presentation after causal readiness.
  - Project histories are home-linear, while resource and agent cardinality are global projections.
    A malformed home fork or cross-project conflict is exposed and fails closed rather than being
    hidden by store transaction order.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
## 2026-08-27 — Rust workspace and dependency guardrails

Created the twelve-crate Rust workspace with a pinned Rust 1.98 toolchain, shared deny-level Rust
and Clippy policy, rustfmt configuration, and the single `hq` binary owned by `hq-node`. Added a
tested, standard-library-only walking skeleton across protocol, domain, application, reducer, and
composition boundaries. Added automated crate inventory, direct-dependency, pure-core,
provider-neutrality, and binary-ownership checks; strict cargo-deny policy; Linux/macOS native CI;
and four-target pure-core checks. Documented the workspace and contributor gates. Formatting,
architecture checks, strict Clippy, Cargo check/build/tests, cargo-deny 0.20.2, all four target
checks, Go build/vet, and the fresh full Go suite pass.

### Original plan entry

- **[foundation/high] Establish the Rust workspace and dependency guardrails** — Add the Cargo
  workspace and initial `hq-domain`, `hq-reducer`, `hq-protocol`, `hq-application`, `hq-store`,
  `hq-local-api`, `hq-relay`, `hq-harness`, `hq-codex`, `hq-tui`, `hq-node`, and `hq-testkit`
  boundaries, initially combining crates only where that improves clarity without weakening
  dependency direction. Configure rustfmt, strict Clippy policy, tests, CI, dependency auditing, and
  architecture checks that keep Tokio, SQLite, Nostr, Ratatui, filesystem, process, and provider
  dependencies out of the pure core. Establish the ADR-0001 Linux/macOS target matrix while keeping
  core crates portable without claiming Windows product support. Add a minimal in-memory walking
  skeleton proving that a domain fact can cross the intended boundaries. Complete this work with a
  clean build/test/lint run and automated forbidden-dependency enforcement.

  Implementation plan:

  - Add a virtual Cargo workspace with the twelve capability-named crates from the architecture,
    a pinned stable toolchain, edition/MSRV/license metadata, shared strict Rust and Clippy lints,
    deterministic development profiles, and no third-party runtime dependency in the initial
    skeleton. Give every crate a documented public boundary and keep `hq-node` as the only
    composition root and owner of the single `hq` binary.
  - Add the smallest typed in-memory vertical slice: `hq-protocol` converts an already trusted
    in-memory frame into an `hq-domain` fact, `hq-application` accepts the fact through a use-case
    boundary, `hq-reducer` derives a projection without I/O, and `hq-node` composes the path. Write
    unit and integration tests before the implementation, including duplicate submission and
    invalid-frame behavior, without pre-empting the next package's full validated-value model.
  - Add `scripts/verify-rust-architecture.sh` first and capture its failure while the workspace is
    absent. Make it verify the exact workspace/crate inventory, shared lint inheritance, the single
    binary owner, direct internal-dependency allowlists, and forbidden runtime/adapter/filesystem/
    process/provider vocabulary in `hq-domain` and `hq-reducer`, plus the one-way
    `hq-codex`-to-`hq-harness` boundary.
  - Add a strict `deny.toml` for advisory, license, duplicate, wildcard, registry, and Git-source
    policy. Extend CI without weakening the frozen Go gates: run Rust format, architecture,
    Clippy, build, and tests natively on Linux and macOS; check the pure core against all four
    ADR-0001 release target triples; and run cargo-deny 0.20.2 on the complete workspace.
  - Document the workspace boundary and contributor verification commands, then run the
    architecture verifier, Cargo metadata/format/check/build/test/Clippy gates, cargo-deny, all
    four core target checks where locally available, whitespace checks, and the unchanged Go
    build/vet/fresh full test suite before archiving this plan entry.

  Risks and decisions:

  - The skeleton uses only standard-library types and deliberately small fact/frame/projection
    shapes. Full bounded values, cryptographic identifiers, catalog payloads, and deterministic
    builders remain owned by the immediately following domain package.
  - Architecture checks enforce both dependency declarations and source-level forbidden imports;
    Cargo's acyclic graph alone cannot distinguish an allowed adapter dependency from accidental
    I/O in the pure core.
  - Linux and macOS native CI are product gates. Cross-target `cargo check` proves portable core
    compilation for x86-64 and ARM64 but does not pretend to exercise target-specific adapters.
  - Dependency policy starts deny-by-default for unknown registries and Git sources while allowing
    the common MIT, Apache-2.0, BSD, ISC, Unicode, and Zlib families expected by later packages.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
## 2026-08-27 — Validated domain primitives and error taxonomy

Replaced numeric skeleton identities with eleven distinct opaque 32-byte ID types and separate
signing/encryption public keys. Added bounded text, vector, root-capable set, and non-empty set
types; typed installation/mailbox addresses; timestamps and revisions; causal parents and
role-specific authority references; provider-neutral operation correlation; resource locators;
command, outcome, page, and versioned-view envelopes; and structured domain errors. Constructors
exclude empty, oversized, duplicate, and unrelated-authority states without I/O, encoding, clocks,
or randomness. Updated the walking skeleton, documentation, and architecture rule accordingly.
Public tests, compile-fail doctest, format, strict Clippy, architecture/spec verifiers, cargo-deny,
all four core targets, Go build/vet, and the fresh full Go suite pass.

### Original plan entry

- **[domain/high] Implement validated domain primitives and error taxonomy** — Replace the walking
  skeleton's placeholder identity and payload shapes with newtyped IDs, public keys, addresses,
  causal references, bounded text and collections, timestamps, correlation values, resource
  locators, generic command/outcome/view envelopes, and typed error categories. Test constructors,
  bounds, non-interchangeability, equality, deterministic ordering primitives, and invalid-state
  exclusion without wire, storage, filesystem I/O, ambient time, or random generation. Complete
  this work when the fact catalog and reducers can depend on validated vocabulary rather than raw
  strings, integers, or byte arrays.

  Implementation plan:

  - Add focused `hq-domain` modules for identifiers and keys, bounded values, time, addressing,
    causal dependency references, correlation, resource locators, command/outcome/view envelopes,
    and structured domain errors. Use private representation, fallible constructors, owned data,
    explicit accessors, and deterministic `Eq`/`Ord` only where the semantics require them.
  - Define distinct fixed-width newtypes for fact, installation, mailbox, account, agent, project,
    message, resource, command, receipt, and operation identities plus public signing/encryption
    keys. Provide byte access without textual parsing or encoding policy; keep secret-key custody,
    signatures, hashing, and serialization outside this package.
  - Define reusable non-empty bounded text and bounded unique collections; an explicit signed
    millisecond timestamp; typed local, account, mailbox, provider/session, operation, and project
    correlation; typed authority roles/references and parent sets; and scheme-tagged resource
    locators that validate their opaque canonical value without touching the filesystem.
  - Write public-contract tests first for empty/oversize/duplicate rejection, type-specific
    address construction, stable ordering, authority-parent consistency helpers, resource scheme
    separation, typed errors, and command/outcome/page behavior. Replace the skeleton's numeric
    fact identity and raw payload construction while keeping its boundary test passing.
  - Document which invariants are enforced now and which belong to protocol verification or the
    following semantic-payload package. Run format, strict Clippy, architecture, cargo-deny,
    Cargo check/build/tests/doctests, four-target pure-core checks, whitespace checks, and the
    unchanged Go build/vet/fresh full suite before archiving this split package.

  Risks and decisions:

  - Fixed-width byte identities are semantic opaque values, not a commitment to a textual or wire
    encoding. Protocol code will later prove content-derived IDs and signatures before constructing
    verified facts.
  - Generic bounded types reject invalid inputs but do not silently normalize Unicode, paths, or
    external provider identifiers; producers must supply the canonical form owned by their
    protocol or adapter.
  - Ordering on IDs and timestamps supports sets, indexes, and the specified presentation tuple;
    it never selects a winner for a concurrent semantic conflict.
  - The 48 catalog payload variants and deterministic test generators remain in the next split
    package so this commit stays reviewable and its primitives can be evaluated independently.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
## 2026-08-27 — Semantic fact payloads and deterministic test support

Replaced the text-only skeleton fact with a verified semantic envelope carrying typed author,
scope, timestamp, causal parents, authority roles, and payload. Added an exact 48-family code
catalog and one typed payload variant per normative FCT row, with canonical/remote-control
isolation and focused intrinsic validation. Code, Markdown, and constructed fixtures prove exact
bidirectional catalog coverage. Added deterministic byte/ID/key streams, explicit clocks, valid
fact builders, all-family payload fixtures, exhaustive small arrival permutations, and
shrink-friendly state-machine sequences. Updated the walking skeleton and documented the boundary.
All Cargo format/check/build/test/doctest/strict-Clippy gates, architecture/spec verifiers,
cargo-deny, four target checks, Go build/vet, and the fresh full Go suite pass.

### Original plan entry

- **[domain/high] Model semantic fact payloads and deterministic test support** — Define a typed
  payload variant for every canonical and remote-control family in the semantic fact catalog using
  only validated `hq-domain` primitives. Build deterministic key, ID, clock, random-byte, fact,
  graph, and state-machine generators in `hq-testkit`, with catalog fixtures and shrink-friendly
  construction. Test complete catalog coverage, payload-specific invalid-state exclusion,
  deterministic generation, and the ability to express every named acceptance scenario without
  raw strings or ambient time/randomness. Complete this work when later reducers and protocol code
  need no ad hoc semantic DTOs or test entropy.

  Implementation plan:

  - Replace the temporary skeleton `Fact` with a verified `SemanticFact` envelope containing an
    opaque fact ID, typed author/scope, explicit timestamp, bounded parents and authority roles,
    and a `SemanticPayload`. Keep signatures, hashes, encoding, receipt metadata, and storage state
    outside the domain envelope.
  - Organize the 48 payload variants into installation/identity, authority/account, conversation/
    activity, agent/session, project, and remote-control modules. Reuse narrow typed records for
    labels, message presentation, lifecycle state, resource health, assignment/runtime outcomes,
    and command stages while retaining a one-to-one `FactKind` and enum variant for every FCT ID.
    Encode required intrinsic exclusions in fallible constructors rather than reducer branches.
  - Add a catalog table in code mapping every `FactKind` to its stable `FCT-NNN` ID, protocol
    class, and retention class. Add a verifier/test that extracts the normative Markdown catalog
    and proves exact bidirectional coverage with no duplicate or invented family.
  - Implement deterministic `hq-testkit` byte/ID/key streams, explicit clock, semantic-fact
    builder, DAG builder, arrival permutations, and small state-machine command sequence builder.
    All generators take an explicit seed/state, produce shrink-friendly ordered data, and expose no
    global random or clock source. Update the walking skeleton to use a catalog payload fixture.
  - Add tests first for catalog coverage, scope/payload matching, required constructor validation,
    deterministic replay and fork behavior, graph parent construction, arrival permutations, and
    enough fixtures to instantiate every acceptance-scenario domain. Document the payload/testkit
    contract and run all Rust, architecture, dependency, target-matrix, whitespace, and unchanged
    Go gates before archiving the package.

  Risks and decisions:

  - The domain catalog records semantic fields and invariants but does not assign JSON keys, enum
    tags, numeric Nostr kinds, signature bytes, SQL columns, or local API shapes.
  - A shared payload record is used only when fields and intrinsic validation are truly identical;
    distinct `FactKind` and `SemanticPayload` variants preserve exhaustive reducer matching.
  - Remote-control payloads live in the same semantic vocabulary but carry a distinct protocol
    class and cannot be mistaken for canonical project-state facts.
  - Testkit output is deterministic test data, not production identity, entropy, cryptography, or
    a promise that generated graphs are semantically usable before reducer validation.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-27 — Causal graph and complete-batch reducer framework

Replaced the reducer summary skeleton with an immutable deduplicating `FactSet`, absorbing identity
collisions, complete parent/reverse indexes including missing vertices, iterative structural and
usable reachability, exact cycle membership, reverse-dependant closure, deterministic dependency
order, and exact usable aggregate frontiers. Added fixed-point normalized decisions with separate
missing and present-unusable blockers, a pure generic domain-reducer seam, transitive projection
support, normalized conflict observations, and the exact typed Kahn presentation comparator. The
application/node walking path now consumes the graph-only complete report. Twelve causal-law and
adversarial tests cover all 64 four-node generated DAGs, merge/permutation/duplicate invariance,
clock reversal, failure propagation, collisions, cycles, every decision class, support, and
non-convergent policy rejection. Workspace format/check/build/test/doctest/strict-Clippy gates,
architecture/spec verifiers, cargo-deny, all four core targets, Go build/vet, and a fresh full Go
test run pass.

### Original plan entry

- **[algebra/high] Implement the causal graph and complete batch reducer framework** — Implement
  immutable fact-set ingestion, deduplication, parent and reverse-dependency graphs, reachability,
  topological processing, unresolved dependency tracking, causal frontiers, projection support,
  normalized reduction decisions, and the single canonical presentation comparator. Expose one pure
  complete-batch reduction entry point and no storage/runtime dependency. Use generated DAGs to
  prove merge semilattice laws, permutation and duplicate invariance, parent-before-child ordering,
  deferred readiness, and exact maximal frontiers. Complete this work when domain reducers can plug
  into a lawful batch engine and no arrival or receiver clock affects semantic output.

  **Implementation plan**

  - Add failing public-API and generated-DAG tests first for exact deduplication, unequal-content ID
    collisions, missing and unusable blockers, present cycles, reverse dependencies, structural and
    usable reachability, exact aggregate frontiers, and deterministic parent-before-child order.
  - Replace the reducer walking skeleton with small pure modules for immutable fact-set ingestion,
    graph indexes, normalized decisions/reasons, domain-stage integration, projection support, and
    presentation ordering. Use ordered collections for normalized output and iterative graph
    algorithms so input iteration order and recursion depth cannot affect results.
  - Define a decoupled domain-stage interface that receives explicit complete-set/graph context and
    returns only closed semantic decisions and typed projection contributions. Provide a permissive
    stage for graph-law tests while preserving a single complete-batch entry point for later
    authority, conversation, agent, and project reducers.
  - Compute readiness and usability to a deterministic fixed point: collisions and cycles fail
    intrinsically, absent parents are listed separately from present-unusable parents, and no
    unusable fact carries causal support. Derive exact usable frontiers and transitive support only
    after decisions stabilize.
  - Implement the reducer-owned Kahn presentation comparator using explicit typed presentation
    keys, retaining causal precedence even when signed clocks move backwards and returning an
    explicit invalid-order error for cyclic selected input.
  - Prove `LAW-MERGE-SET-UNION`, `LAW-INPUT-INVARIANCE`, `LAW-CAUSAL-DOMINANCE`,
    `LAW-EXACT-MAXIMAL-FRONTIERS`, and `LAW-DEFERRED-READINESS` across deterministic generated DAGs,
    arrival permutations, duplicates, and clock-skew cases. Document the public framework and its
    boundary from protocol, storage, runtime, and receiver clocks.
  - Run formatting, workspace check/build/test/doctests, strict Clippy, architecture verification,
    dependency policy, and the retained Go regression suite before recording the package.

  **Risks and mitigations**

  - A one-pass domain callback could make a later conflict or revoke input-order-sensitive; expose
    complete-set context and repeat domain classification to a stable normalized result.
  - A graph-only topological order could accidentally choose a domain winner; keep presentation
    ordering and semantic admission as separate typed operations and never use timestamps or IDs as
    authority/conflict rules.
  - Collision and cycle members cannot safely support descendants; represent both with closed reason
    codes and propagate them as present-unusable blockers rather than silently dropping them.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including the repository-wide gates named
     above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Peer, capability, and human-account reduction

Added exact catalog authority roles and a pure `AuthorityReducer` with explicit local policy,
normalized installation, mailbox, peer-route, capability, account, membership, and selection
projections, exact causal support, and closed decisions. The reducer enforces signer, scope, role,
subject, audience, unique-root, remove-wins, owner-observation, historical-authority, and full-frontier
regrant/reaccept rules. Added causal fixtures and executable coverage for `AUTH-001` through
`AUTH-022`, including all 5,040 arrival orders for the mailbox race. Workspace format, architecture
and spec verification, check/build/test/doctest/strict-Clippy, cargo-deny, four-target core checks,
whitespace, and the complete Go build/vet/fresh-test regression suite pass.

### Original plan entry

- **[authority/high] Implement peer, capability, and human-account reduction** — Add pure reducers
  for installation-local identity/binding facts, directional peer routing, mailbox access grants,
  observations and revokes, human-account creation, device grants, acceptances, revocations,
  selection, and membership frontiers. Authorization must use explicitly cited historical facts at
  the action's causal point. Prove that observed pre-revoke actions survive, concurrent or later
  unauthorized traffic fails closed, and a regranted device becomes authoritative only through a
  causal-maximal acceptance descending from the revoke. Cover missing authority, conflicting roots,
  every topological arrival order, and unrelated-parent attacks. Complete this work when the full
  authority race matrix and batch-reduction laws pass.

  **Implementation plan**

  - Add failing authority fixtures and public-contract tests first for installation and mailbox
    roots, exact local signer/scope rules, peer route block/restore frontiers, directional mailbox
    grants, owner observations, grant revocation, human-account roots, device grant/accept/revoke,
    local account selection, and authorization of later peer/account-scoped fact families.
  - Introduce typed authority aggregate keys, closed rejection/conflict reasons, and normalized
    projections for installations, mailboxes, peer routes, capability lineages, human accounts,
    device memberships, and local selections. Every active projection will retain exact support and
    every multivalue or unique-root conflict will expose all participants.
  - Validate required parent kinds, typed authority roles, signer, subject, audience, and scope
    independently; never infer authority from an ordinary ancestor, peer route, current display
    state, relay metadata, or a fact ID/timestamp ordering. Treat wrong signer/subject relationships
    as invalid and available-but-insufficient historical authority as unauthorized.
  - Derive peer routing as a remove-wins register: concurrent block beats route set, a restore must
    descend from every maximal block, and unrelated descendants cannot clear a block. Keep route
    history visible while emitting no routable singleton for a conflicted or blocked frontier.
  - Derive mailbox capability history at each action's causal point. Require actions to cite the
    exact matching grant, preserve only actions made usable before an owner-signed observation that
    a revoke cites, reject concurrent/post-revoke old-grant actions, and require a regrant lineage
    to descend from every maximal prior revoke.
  - Derive human membership from one account creator root plus exact target-key acceptance. Apply
    remove-wins across every causal-maximal acceptance/revoke, require post-revoke grant and
    acceptance lineages to descend from all revoke maxima, and accept account-scoped actions only
    through the creator or one active maximal acceptance for the exact account.
  - Exhaust every topological arrival order for the named `AUTH-001` through `AUTH-022` race shapes,
    plus duplicates, conflicting roots, missing parents, partial frontiers, changed payload/key,
    wrong account/direction, and unrelated-parent attacks. Re-run all reducer laws to prove the
    authority stage preserves complete-batch input invariance and projection retraction.
  - Document the public authority model and run format, workspace check/build/test/doctests, strict
    Clippy, architecture/spec verification, dependency policy, four-target core checks, whitespace,
    and the unchanged Go build/vet/fresh full regression suite before recording the package.

  **Risks and mitigations**

  - Revocation is non-monotone in projections even though knowledge is add-only; compute authority
    from the complete usable graph on every batch and include revokes/observations in aggregate
    membership so fixed-point reclassification retracts affected descendants.
  - A historical acceptance can remain structurally maximal in a partial lineage; require the exact
    grant/accept payload and signer match and compare against every maximal revoke before granting
    account authority.
  - Base authority must remain reusable by conversation, activity, project, and remote-control
    reducers; keep policy in a focused pure module with typed normalized outputs and no dependency
    on those packages' projection rules.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named authority scenario
     and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.


## 2026-08-27 — Conversation and activity reduction

Added a pure authority-composed `ConversationReducer` with normalized question/async threads,
independent answer/cancellation relation matrices, stable message-ID conflicts, remove-wins
archive/restore, absorbing rejection, causal peer-receipt evidence, typed action groups and final
answers, and inert incomplete addressed observations. Completed the typed activity payload and
implemented source/provider/session/operation/item/runtime namespaces, semantic-sequence winners,
explicit sequence/runtime conflicts, durable completed history, and deterministic newest-200 progress
retention. All `CONV-001`–`CONV-016`, `ACT-001`–`ACT-009`, and `REG-002` cases execute,
including exhaustive small permutations and a 205-record rebuild. All Rust workspace, strict-Clippy,
architecture/spec, cargo-deny, four-target core, whitespace, and Go regression gates pass.

### Original plan entry

- **[conversation/high] Implement conversation and activity reduction** — Add questions, answers,
  asynchronous messages, cancellation, archive/restore/reject, delivery-relevant semantic state,
  typed presentation/correlation, and the separate non-actionable harness-activity stream. Define
  one reducer-owned causal ordering comparator and deterministic activity coalescing/retention rules;
  no store or UI may recreate them. Test missing parents, concurrent answer/cancellation, equal-time
  messages and activity, delayed occurrence data, final-answer selection, action grouping, and
  projection retraction. Complete this work when normalized conversation and activity views are
  deterministic for all generated arrival orders.

  **Implementation plan**

  - Add failing public-contract fixtures and tests first for every named `CONV-001` through
    `CONV-016`, `ACT-001` through `ACT-009`, and `REG-002` scenario. Generate small answer,
    cancellation, message-state, and activity graphs across every topological arrival order,
    duplicates, late parents, equal authored/occurrence times, clock reversal, and projection
    retraction.
  - Complete the typed harness-neutral activity value model needed by the retained contract:
    provider/session/operation plus optional item correlation, runtime-lifetime identity, signed
    occurrence time, positive source sequence, kind/status, bounded content, and explicit
    truncation. Keep message purpose, presentation kind, public ID, and operation grouping typed;
    prove that prose imitating authority, correlation, or final-answer markers is inert.
  - Add a focused pure conversation/activity reducer that composes the existing authority policy
    without duplicating its rules. Introduce closed reasons, typed aggregate/projection keys, exact
    support, incomplete addressed observations, and normalized thread, message-state, delivery,
    action-group, final-answer, activity-history, activity-winner, collision, and unified-entry
    views.
  - Validate exact root/child/state target kinds, derived thread identity, sender/recipient reversal,
    compatible scope and correlation, required causal ancestry, controlling mailbox/account, and
    complete state frontiers. Treat unequal stable message IDs as explicit conflicts; retain answers
    and cancellations independently and expose every before/after/concurrent relation.
  - Implement archive/restore as a remove-wins register over causal maxima and rejection as an
    absorbing tombstone. A restore opens only after every maximal archive and never after rejection;
    state facts remain auditable while open/action projections retract and exact frontier/support
    changes are visible.
  - Derive peer-received evidence only from a usable peer-authored child that cites the outbound
    message, never from relay or receipt metadata. Select ready answers and typed final answers only
    through the reducer-owned canonical presentation traversal, retaining all candidates and using
    operation correlation as the action group.
  - Keep activity in a disjoint non-actionable stream. Coalesce snapshot/progress facts only within
    the full source mailbox, provider, session, operation, kind, item, and runtime namespace; choose
    higher semantic sequence across concurrent snapshots, report equal-sequence unequal-content or
    conflicting-runtime collisions, retain completed items as history, and deterministically keep
    the newest 200 progress winners per source/provider session while canonical facts remain intact.
  - Feed all projected messages and retained activity through the sole typed Kahn comparator,
    including occurrence and correlation tie breakers while preserving parent-before-child order.
    Document the normalized model and run format, architecture/behavior/spec verifiers, workspace
    format/check/build/test/doctests, strict Clippy, dependency policy, four-target core checks,
    whitespace, and unchanged Go build/vet/fresh full regression suite before recording the package.

  **Risks and mitigations**

  - Conversation classification depends on historical authority but must also expose domain-specific
    decisions and projections; factor reusable authority-stage helpers and wrap their closed reasons
    rather than copying or weakening signer, scope, membership, and revocation checks.
  - Stable presentation order and activity winner selection solve different problems; use semantic
    sequence only inside an exact activity aggregate, then order retained entries with the canonical
    comparator so timestamps or fact IDs never decide a conflict.
  - Incomplete addressed content is intentionally observable before it is usable; expose it through
    a separate inert projection that cannot support a thread, action, delivery, archive, answer, or
    final-answer view until the missing causal history projects.
  - Activity retention is a disposable-view policy over permanent canonical facts; compute the
    budget from complete normalized winners with a total typed key so batch, late-parent replay, and
    repair rebuilds select exactly the same 200 rows.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named conversation and
     activity scenario and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Named-agent and provider-session reduction

Implemented a pure, authority-composed named-agent reducer with permanent name reservations,
immutable provider-session bindings, repository-context history, causal selection and rename
registers, absorbing retirement, and projectless direct-session projections. Added executable
coverage for every `AGT-001` through `AGT-010` scenario, including permutation and duplicate replay,
and documented the public model. The complete Rust workspace, architecture/behavior/spec verifiers,
strict Clippy, dependency policy, all four core targets, whitespace checks, and unchanged Go
build/vet/fresh regression suite pass.

### Original plan entry

- **[agents/high] Implement named-agent and provider-session reduction** — Add pure facts and
  projections for mailbox creation/binding/context, permanent name claims and retirement, durable
  provider-session bindings, selection, renaming, repository context, and projectless direct
  sessions. Keep durable session identity separate from runtime presence, leases, caller
  environments, and process state. Define name/session conflicts and replay behavior explicitly and
  test rebuildable history, retirement, reselection, and cross-provider namespace isolation.
  Complete this work when all retained named-agent state derives solely from the fact set.

  **Implementation plan**

  - Add failing fixtures and public-contract tests first for every named `AGT-001` through
    `AGT-010` scenario, then generate small claim/binding/context/selection/rename/retirement graphs
    across every arrival order, duplicates, late parents, partial frontiers, and clock reversal.
  - Add a pure named-agent reducer that composes historical authority without duplicating it and
    emits closed reasons, typed aggregate/projection keys, exact support, permanent name
    reservations, mailbox/session histories, context frontiers, selection/rename registers,
    retirement state, and projectless direct-session views.
  - Validate installation-local signer/scope, exact agent mailbox roots, lowercase name syntax,
    stable agent/name/mailbox subjects, typed claim and binding parents, provider/session namespace,
    selected immutable repository context, and complete selection/rename frontiers independently.
    Repository context remains display/search metadata and grants no authority.
  - Treat one name, agent ID, or agent mailbox claimed incompatibly as an explicit permanent
    conflict. Keep a retired name reserved forever, expose every participant, and never use authored
    time, fact ID, arrival, or current runtime state to select a claimant.
  - Treat provider plus session as one immutable binding identity: rebinding it to another mailbox
    conflicts, one mailbox may retain several distinct sessions, and equal session text in different
    providers remains isolated. Retain unnamed projectless mailbox/session history without
    inventing a named runnable agent.
  - Derive selection as a multivalue causal register. Concurrent distinct selections expose every
    maximum and block runnable selection; one later selection resolves only when it descends from
    every prior maximum and cites the exact name claim, binding, and matching repository context.
  - Derive per-session display rename as an independent multivalue register with sorted candidates,
    explicit clear, exact frontier/support, and no effect on selection or runtime. Retain all
    mailbox context history and every causal-maximal context value.
  - Make retirement absorbing and remove-wins against concurrent selection/rename state. Historical
    sessions, names, contexts, and selections remain queryable, but no retired/conflicted agent is
    runnable and no post-retirement session fact can reactivate it. Prove normalized output contains
    no process, lease, presence, phase, caller environment, or ambient filesystem state.
  - Document the public named-agent/session model and run format, architecture/behavior/spec
    verifiers, workspace format/check/build/test/doctests, strict Clippy, dependency policy,
    four-target core checks, whitespace, and unchanged Go build/vet/fresh full regression suite
    before recording the package.

  **Risks and mitigations**

  - Authority already admits installation-local binding families but does not own their global
    uniqueness or agent semantics; reuse its classification as the first stage, then apply focused
    name/session rules without altering prior authority projections.
  - Selection facts embed repository context while context facts are grow-only and may be
    concurrent; require an exact projected mailbox-context value cited in the selection lineage and
    expose context ambiguity instead of choosing a timestamp winner.
  - Retirement is non-monotone only in runnable projections; recompute from the complete usable
    history so late retirement retracts active selection while immutable facts and permanent name
    reservation remain.
  - Direct unnamed sessions and named managed agents share binding history but not lifecycle; keep
    distinct typed projections so a mere binding never synthesizes a claim, selection, or runnable
    worker.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named agent/session
     scenario and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Pure project and resource-claim model

Implemented the pure home-linear project reducer, explicit lifecycle/archive/resource transition
model, pluggable path-claim policy, global assignment cardinality, exact agent/session and thread
binding, contiguous input sequencing, at-most-once dispatch attribution, current/late output
classification, and isolated remote-command stages. Added grouped executable coverage mapping
every PRJ-001 through PRJ-023 and CTL-001 through CTL-004 scenario and documented the public
model. The complete Rust workspace, architecture/behavior/spec verifiers, strict Clippy,
dependency policy, all four core targets, whitespace checks, and unchanged Go build/vet/fresh
regression suite pass.

### Original plan entry

- **[projects/high] Implement the pure project and resource-claim model** — Add project identity and
  immutable home, mailbox, metadata, predecessor, desired resources, primary path, lifecycle,
  archive state, active claims, assignment epochs, thread scope, project input sequencing, dispatch
  attribution, expected-head compare-and-swap, remote command/result state, and late-output
  classification. Model reversible domain transitions separately from operational saga states and
  keep resource-kind policy behind explicit pure interfaces. Test stale heads, concurrent commands,
  assignment cardinality, close/reopen/archive laws, force-takeover authority, and inactive-output
  behavior. Complete this work when the project transition model satisfies every invariant in the
  retained project specification without filesystem or provider I/O.

  **Implementation plan**

  - Add failing public-contract fixtures first for every named `PRJ-001` through `PRJ-023` and
    `CTL-001` through `CTL-004` acceptance scenario. Generate small home-linear histories and
    global project sets across arrival permutations, duplicates, late parents, partial frontiers,
    stale siblings, and authored-clock reversal.
  - Add a pure project reducer that composes historical authority plus named-agent and conversation
    projections without copying their rules. Emit closed decisions, typed aggregate/projection
    keys, exact support and blockers, immutable project identity, unique home/mailbox roots,
    complete accepted history, authoritative head/frontier, and explicit fork participants.
  - Express the home transition algebra as pure functions over typed state. Creation establishes
    immutable home, mailbox, predecessor, metadata, desired resources, optional primary path,
    lifecycle, archive state, and input sequence; every later canonical project fact must cite the
    exact unique head, and sibling or stale children remain visible without becoming a winner.
  - Separate stable lifecycle from operational preparation/closing/configuring states. Enforce
    atomic reopen, resource replacement, activation compensation, close, force-close, reopen,
    archive, and unarchive laws; archive requires closed and unassigned state, unarchive yields a
    visible closed claim-free project, and runtime observations never assert external cessation.
  - Put resource overlap behind a pure policy interface. For first-release path resources compare
    home-qualified canonical locators for equality or ancestor/descendant overlap, permit overlap
    within one project and equal spelling across homes, activate all desired claims atomically, and
    expose every cross-project conflict without using fact ID, timestamp, or arrival as a winner.
  - Model assignment epochs explicitly from configuring through runnable, blocked, and ended.
    Enforce at most one active agent per project and one active project per agent, exact selected
    immutable project-thread scope, provider-session binding, launch context, graceful/forced end
    authority, conflict retraction, and restoration when a competing epoch validly ends.
  - Derive one contiguous home input sequence and immutable at-most-once dispatch attribution.
    Validate the exact accepted project message, current runnable assignment, agent, scoped thread,
    provider session, and sequence; expose duplicate ID/sequence/dispatch conflicts rather than
    choosing a branch.
  - Retain project output by stable ID and complete provenance. Deduplicate identical retries,
    conflict any changed body/presentation/correlation/binding, classify output against the complete
    assignment history as current or late-from-inactive, and never allow output to mutate lifecycle,
    claims, assignment, or dispatch authority.
  - Derive remote command views independently from canonical project state: active-device requests
    queue only, home receipts record the observed head, committed outcomes cite canonical descendant
    facts, rejected outcomes retain typed stale/current-head and runtime certainty, and unequal
    receipt or terminal values conflict without a selected result.
  - Document the public project/resource model and run format, architecture/behavior/spec
    verifiers, workspace format/check/build/test/doctests, strict Clippy, dependency policy,
    four-target core checks, whitespace, and unchanged Go build/vet/fresh full regression suite
    before recording the package.

  **Risks and mitigations**

  - Home-linear validity and global safety can retract different projections; compute accepted
    history first, then derive project state, path conflicts, assignment cardinality, dispatch, and
    output status as separate deterministic passes with explicit cross-pass inputs.
  - A generic workflow framework would obscure project-specific transition laws; keep a small typed
    transition function and explicit per-fact validation, sharing only proven graph/frontier and
    multivalue helpers.
  - Resource identity in this package is semantic rather than filesystem-derived; accept only typed
    canonical locators and delegate materialization, symlink revalidation, health, and release
    assessment to later adapters while proving those observations cannot change lifecycle.
  - Remote control and operational saga checkpoints are observable but not competing project-state
    authorities; project only home-signed canonical facts and retain command/runtime uncertainty in
    disjoint views.
  - The payload catalog is intentionally large; keep normalized state and view structs bounded and
    keyed, factor validation by semantic family, and use focused generated fixtures to prevent one
    monolithic reducer path from hiding invariant gaps.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named project/control
     scenario and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Canonical fact, remote-control, and trust specifications

Specified independent `hq/canonical` v1 and `hq/control` v1 protocols with strict canonical UTF-8
JSON, named raw/decoded/encoded bounds, signed scope and typed cross-namespace causal references,
exact NIP-01 event construction, and provisional regular kind 6000 selected through a revision-pinned
ADR. Published an exhaustive owned DTO mapping for all 48 semantic families and an explicit trust
state/failure model that prevents malformed, failed, or verified-unsupported input from exposing a
semantic fact. Added exact canonical and control vectors whose preimages reproduce their SHA-256 IDs
and whose BIP-340 signatures passed both `nak 0.20.2` and the independent btcsuite Schnorr verifier,
plus a machine-readable adversarial corpus and consistency/link/vector-integrity checks. All Rust
workspace, strict-Clippy, architecture/behavior/causal/protocol-spec, cargo-deny, four-target core,
whitespace, and unchanged Go build/vet/fresh regression gates pass.

### Original plan entry

- **[protocol/high] Specify canonical facts, remote control, and trust transitions** — Write
  canonical fact v1 and remote-control v1 as new protocols with independent version spaces,
  deterministic encoding rules, strict decoding policy, size/count/text bounds, event identity,
  signatures, audience and authority representation, unsupported-version behavior, and exact trust
  transitions from raw bytes to verified semantic facts. Decide the provisional Nostr application
  kind and encoding using an ADR rather than inheriting Go values. Define exact vectors and
  adversarial cases before implementation. Complete this work when every semantic fact has an
  unambiguous DTO mapping and no domain struct accidentally serves as a wire schema.

  **Implementation plan**

  - Verify the current primary NIP-01/NIP registry requirements for event serialization, identity,
    Schnorr signatures, application-kind ranges, and extensibility. Record the checked revisions
    and write an ADR selecting one provisional immutable application kind plus its compatibility
    and registration posture without inheriting any Go kind or schema.
  - Specify two independent versioned namespaces: canonical fact v1 for `FCT-001` through
    `FCT-045`, and remote-control v1 for `FCT-046` through `FCT-048`. Give each an explicit media
    shape, protocol discriminator, version field, supported-family registry, typed ID namespace,
    and unsupported-version/family retention behavior.
  - Define the exact UTF-8 JSON wire grammar and one canonical byte form: object member order,
    integers, booleans, null/omission, string escaping, Unicode policy, arrays, duplicate and unknown
    members, trailing data, depth/count limits, and rejection of semantically equal non-canonical
    spellings. Bounds apply to decoded semantic values and to final encoded bytes after escaping.
  - Define NIP-01 event construction independently from the payload DTO: exact application kind,
    fixed tag vocabulary/order, empty-versus-present tag rules, content bytes, event serialization,
    SHA-256 identity, 32-byte lowercase hex, BIP-340 signing, signature verification, public-key
    agreement, and preservation of the exact verified event and content bytes.
  - Specify signed scope/audience DTOs and typed causal references. Encode parents as a sorted unique
    list and authorities as sorted unique role/fact pairs whose IDs also occur in parents; define
    canonical and remote-control cross-namespace reference rules and reject unknown roles, duplicate
    roles, role/parent mismatch, and audience/author contradictions before semantic construction.
  - Publish an exhaustive mapping table from every semantic payload field and nested enum/value to
    an owned protocol DTO field, including numeric catalog family IDs, bounded text/collections,
    optional values, timestamps, nonzero sequences, repository/resource locators, messages,
    activity, agent/session, project/assignment/input/output, and remote command/result/runtime
    variants. Domain enum or Rust field spelling is never normative wire vocabulary.
  - Define the trust-state machine and failure taxonomy from untrusted raw event bytes through
    bounded outer parse, canonical event verification, exact content retention, protocol dispatch,
    bounded DTO parse, canonical re-encoding equality, semantic conversion, and reducer admission.
    Raw, parsed, cryptographically verified, verified-supported, verified-unsupported, and semantic
    values remain distinct and no failed or unsupported state exposes a `SemanticFact`.
  - Add exact hand-checkable canonical and remote-control vectors with payload bytes, NIP-01 event
    preimage, event ID, public key, signature, and expected semantic mapping, plus adversarial corpora
    for malformed JSON, escaping, duplicate/unknown fields, ordering, bounds, invalid hex,
    wrong kind/version/family, namespace confusion, tampering, bad signatures, and authority/scope
    mismatch. State which independent implementation or standard vector validates crypto values.
  - Add machine-readable protocol-spec consistency tests that prove all 48 catalog families appear
    exactly once in the mapping/registry, protocol ranges remain disjoint, every bound is named,
    vectors are exact files rather than prose ellipses, and ADR/spec links are complete.
  - Run documentation format/link checks, architecture/behavior/causal-spec verifiers, workspace
    format/check/build/test/doctests, strict Clippy, dependency policy, four-target core checks,
    whitespace, and unchanged Go build/vet/fresh full regression suite before recording the package.

  **Risks and mitigations**

  - Nostr kind registration and NIP text can change; cite the exact upstream revision reviewed,
    select a provisional regular-event kind through the ADR, and isolate the kind as outer carriage
    so a future registered value does not silently change canonical payload v1 bytes.
  - Generic JSON libraries accept multiple spellings and usually lose duplicate/order information;
    specify validation over exact retained bytes and canonical re-encoding equality before choosing
    an implementation library in the next package.
  - One shared version field could couple immutable facts to remote workflow evolution; keep two
    discriminators and version registries even though both ride the same signed-event boundary.
  - Exhaustive payload mapping is large and typo-prone; key it to stable `FCT-xxx` numbers, generate
    consistency assertions from the domain catalog, and require a named DTO for every nested sum
    type instead of an untyped payload map.
  - A cryptographically valid event is not necessarily an authorized or meaningful HQ fact; make
    each trust transition explicit and preserve verified unsupported input without allowing it into
    semantic conversion or reduction.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the specification in proportion to risk, including registry/mapping consistency,
     exact vectors, adversarial cases, and every repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so specifications, tests, and plan bookkeeping form one reviewable
     change.

## 2026-08-27 — Signed-event cryptographic trust boundary

Implemented bounded exact HQ NIP-01 event parsing and encoding, SHA-256 event identity, raw
32-byte BIP-340 signing and verification, retained raw/preimage/content bytes, and disjoint
raw, parsed, cryptographically verified, supported, and verified-unsupported owners. Added a
specialized canonical JSON cursor and closed redacted failures for malformed wire shapes, limits,
wrong kind/tags, tampering, invalid keys/signatures, authored-time disagreement, namespace
confusion, unsupported prefixes, and frozen Go schemas. Published tests for both signed-event
vectors and selected official BIP-340 valid/invalid vectors, a compile-fail trust-state proof,
adversarial boundary coverage, and a seeded raw-byte cargo-fuzz smoke gate pinned to cargo-fuzz
0.12.0 and nightly-2026-08-26. `k256` 0.14 and `sha2` 0.11 remain pure Rust and compile on all
four release triples; root and isolated-fuzz dependency policies, every Rust workspace/spec/
architecture gate, whitespace, and unchanged Go build/vet/fresh regression suite pass.

### Original plan entry

- **[protocol/high] Implement strict signed-event framing and the cryptographic trust boundary** —
  Implement bounded exact NIP-01 outer-event parsing/encoding, SHA-256 identity, BIP-340
  signing/verification, retained raw/preimage/content bytes, distinct raw/parsed/verified types, and
  bounded protocol-prefix dispatch into supported content or verified-unsupported records. Reject
  wrong kind/tags, non-canonical outer JSON, tampering, bad keys/signatures, time disagreement, and
  old Go schemas before DTO or reducer access. Complete this split package when the published event
  vectors and independent BIP-340 vectors pass and no unverified value can call a verified API.

  **Implementation plan**

  - Add failing public API tests first for the two published signed-event vectors, selected official
    BIP-340 valid/invalid vectors, exact byte retention, each trust-state constructor boundary,
    deterministic preimage/ID reconstruction, and an explicit signer supplied auxiliary randomness.
  - Add narrowly owned protocol dependencies at current reviewed releases: pure-Rust `k256` Schnorr
    verification/signing and `sha2` hashing, with default features minimized, workspace dependency
    policy updated, licenses audited, and four-target compilation retained. Keep key material out of
    domain types and ensure signer/debug/error surfaces never expose a secret.
  - Replace the walking-skeleton-only boundary with immutable `RawEventBytes`, `ParsedOuterEvent`,
    `CryptographicallyVerifiedEvent`, `SupportedContentBytes`, and
    `VerifiedUnsupportedRecord` owners. Preserve the walking skeleton only as an explicitly
    non-normative compatibility path until later application work removes it.
  - Implement a specialized allocation-bounded JSON cursor for the exact seven-member outer object
    and NIP-01 string escaping. Enforce member order, unknown/duplicate/missing rejection, minimal
    integer and escape spellings, valid UTF-8/scalars, empty tags, no whitespace/trailing bytes, and
    raw/content limits before copying attacker-controlled data.
  - Encode the exact event-ID preimage and outer event without a generic JSON value. Recompute
    SHA-256 before signature verification, compare IDs without data-dependent early exit, parse
    x-only keys/signatures canonically, verify BIP-340 over the 32-byte event ID, and retain exact
    raw, reconstructed preimage, and decoded content bytes.
  - Add a signer boundary that accepts a validated secret-key owner plus caller-supplied
    cryptographic auxiliary randomness, derives the x-only public key, signs the precomputed event
    ID exactly once, self-verifies, zeroizes key material through its crypto owner, and produces the
    same immutable verified representation used by inbound events.
  - Implement bounded prefix dispatch for the exact ordered `p`, `v`, and `f` content fields.
    Distinguish supported canonical/control content, verified unsupported protocol/version/family,
    namespace confusion, and malformed prefixes without constructing payload DTOs or semantic facts.
  - Add boundary and adversarial tests for zero/maximum lengths, one-byte-over limits, malformed
    UTF-8/JSON/escapes/hex/integers, reordered/duplicate/unknown members, nonempty tags, wrong kind,
    ID/content/signature tampering, invalid curve points and signature scalars, namespace confusion,
    unsupported values, and legacy Go event/schema samples. Prove failure values expose no verified
    content and unsupported values expose no supported content.
  - Add a raw-byte cargo-fuzz target and seeded corpus for outer parse/verify/dispatch; build it in a
    pinned short smoke gate and document longer sanitizer runs. Run format, all spec/architecture
    verifiers, workspace check/build/test/doctests, strict Clippy, dependency policy, four-target
    core checks, whitespace, and unchanged Go build/vet/fresh regression suite before recording.

  **Risks and mitigations**

  - A prehash/signing-trait mismatch could silently sign SHA-256(event-ID) instead of the NIP-01
    event ID; use the explicit prehash interfaces, official BIP-340 vectors, published HQ vectors,
    and independent verifier results to pin the exact 32-byte message semantics.
  - Generic JSON parsing can normalize duplicates, ordering, numbers, or escapes before policy sees
    them; keep the small outer grammar in a byte cursor and require deterministic re-encoding where
    decoded strings are involved.
  - Unsupported content must be retained only after cryptographic proof but must not be mistaken for
    a valid DTO; use disjoint types with no shared semantic conversion method and classify only a
    bounded canonical prefix.
  - Signing APIs can accidentally make secrets cloneable or printable; wrap the zeroizing crypto
    key, omit `Clone`/`Debug`/serialization, take explicit auxiliary randomness, and return closed
    redacted errors.
  - Fuzz tooling uses nightly and may not exist on a contributor machine; keep deterministic corpus
    regression tests on stable, pin cargo-fuzz for CI/smoke use, and treat longer fuzz duration as an
    additive security gate rather than replacing ordinary tests.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every vector, boundary,
     adversarial case, fuzz smoke, and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, fuzz assets, and plan bookkeeping form one reviewable
     change.

## 2026-08-27 — Canonical v1 owned DTO catalog

Implemented the complete owned `hq/canonical` and `hq/control` v1 DTO catalog for all 48 numeric
families, including fixed lowercase-hex values, required nullable fields, exact tagged scopes and
references, every nested object/sum type, named decoded bounds, positive sequences/timestamps,
family scope and authority-role applicability, sorted unique parents/authorities, authority-parent
linkage, and core representational invariants. Full decoding prevalidates bounded canonical JSON,
uses typed Serde 1.0.229 DTOs plus only `serde_json::RawValue` body isolation, rejects unknown/
duplicate/missing fields, deterministically re-encodes with serde_json 1.0.151, and advances to
`VerifiedSupportedRecord` only after byte equality with retained signed content. Added executable
exact round trips for every family and both published vectors, adversarial nested/ordering/
namespace/enum/hex/boundary coverage, inclusive named text-bound tests, and a second re-signing
cargo-fuzz target seeded by canonical and control contents. Both fuzz targets, root and isolated
dependency policies, all four protocol targets, every Rust workspace/spec/architecture gate,
whitespace, and unchanged Go build/vet/fresh regressions pass.

### Original plan entry

- **[protocol/high] Implement strict canonical v1 DTO decoding and encoding** — Implement the
  complete owned canonical/control v1 DTO catalog, strict full-content decoding, deterministic
  encoding, duplicate/unknown/missing/reordered-field policy, enum and fixed-width primitive
  vocabulary, decoded and post-escaping bounds, sorted reference representation, and a distinct
  verified-supported DTO trust state. Add exhaustive round-trip fixtures for all 48 families,
  independent exact vectors, malformed/non-canonical/boundary corpora, and structure-aware fuzzing.
  Complete this split package when every normative DTO shape is executable and no non-canonical or
  merely prefix-supported input can acquire a fully verified DTO.

  **Implementation plan**

  - Add failing public-contract tests first for the two published vector contents and a table with
    one complete valid content record for every family 1 through 48. Assert exact family/namespace
    dispatch, owned DTO variant selection, byte-for-byte re-encoding, exact verified-event
    retention, and a disjoint `VerifiedSupportedRecord` state reached only from
    `SupportedContentBytes`.
  - Add current reviewed `serde` 1.0.229 with derive and `serde_json` 1.0.151 with only the required
    standard/raw-value features to `hq-protocol`. Use them only for statically typed owned DTOs and
    raw body isolation; never construct or expose `serde_json::Value`, maps, or domain serialization.
    Audit the expanded graph and retain compilation on all four release targets.
  - Define one exact common content envelope and owned protocol types for fixed hex, required
    nullable properties, scope arrays, namespace-qualified parents, role-qualified authorities,
    locators, contexts, operations, messages, resources, bindings, activity/runtime/result sums,
    and every family body. Keep wire names and enum spellings in `hq-protocol`, independent of Rust
    domain field names.
  - Reuse the bounded canonical JSON prevalidator before Serde allocation. Deserialize the common
    envelope with `deny_unknown_fields` and a retained raw body, require all nine properties,
    cross-check discriminator/version/family against the consumed prefix state, then deserialize
    the body into the exact family DTO. Reject duplicate, missing, unknown, wrong-type, overflow,
    invalid UTF-8/scalar, floating-point, negative, and trailing input before producing a verified
    DTO.
  - Validate decoded primitives and collection representation without semantic construction:
    lowercase 32-byte hex, nonempty named text bounds, positive sequences, signed-millisecond
    range, closed enum spellings, object/array limits, unique relay/resource identities, sorted
    unique namespace-qualified parents, sorted unique authority triples, authority-as-parent, legal
    canonical/control reference directions, and family-applicable authority-role vocabulary.
  - Serialize only from owned DTO structs in normative member order with every optional property
    present as a value or `null`. Enforce final content bounds after escaping and require the result
    to equal the retained verified content bytes before constructing `VerifiedSupportedRecord`;
    classify every semantically equal alternate spelling or member order as non-canonical.
  - Provide a deterministic outbound DTO encoder that performs the same validation and size checks
    before yielding bytes suitable for the existing signer, while retaining the DTO owner for the
    following semantic-conversion package. Keep signing, authorization, and reduction outside the
    DTO module.
  - Add adversarial tests covering every common/nested shape, required-null omission, duplicate and
    unknown properties at each depth, reordered members, nonminimal escapes and numbers, bad hex,
    unknown enums, wrong body/family pairs, invalid reference namespaces/order/roles, zero/overflow
    sequence/time values, duplicate/oversized collections, decoded text limits, and one-byte-over
    final escaped content. Include frozen Go schema samples and prove failures expose no DTO.
  - Extend the isolated cargo-fuzz workspace with a content target seeded by both published vectors.
    Re-sign bounded mutations with the fixture key so parse/dispatch/DTO validation remains
    reachable, run the pinned smoke gate, and document longer sanitizer runs. Run format, all
    spec/architecture verifiers, workspace check/build/test/doctests, strict Clippy, root and fuzz
    dependency policy, four-target protocol checks, whitespace, and unchanged Go build/vet/fresh
    regression suite before recording.

  **Risks and mitigations**

  - Serde normally accepts input spellings and member orders that v1 forbids; treat typed decoding
    as provisional, serialize the exact DTO back into the normative order, and advance trust only
    after byte equality with retained content.
  - `Option<T>` can make a missing property indistinguishable from required `null`; use an owned
    required-nullable wrapper or equivalent visitor so omission is rejected before canonicality
    comparison.
  - A generic body value or untagged enum could normalize duplicates or select an ambiguous shape;
    retain only the raw body slice, dispatch by the already verified numeric family, and deserialize
    exactly one named body type.
  - Forty-eight mappings invite drift and copy/paste gaps; drive the registry from one exhaustive
    numeric match, table-test every family, and add bidirectional consistency assertions against
    `FactKind::ALL` and the normative mapping document.
  - Structure-aware fuzzing cannot mutate a signature-covered payload directly; re-sign only inside
    the isolated harness with a published fixture secret and explicit deterministic auxiliary bytes,
    never add a production bypass around cryptographic verification.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including all 48 family fixtures, every
     malformed/boundary class, fuzz smoke, and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so DTO code, tests, fuzz assets, and plan bookkeeping form one
     reviewable change.

## 2026-08-27 — Verified DTO semantic conversion

Implemented the sole reducer-ready transition from a complete `VerifiedSupportedRecord` into
`VerifiedSemanticFact`, retaining exact signed content and event evidence alongside the domain
fact. Exhaustive conversion covers all 48 canonical/control families, every nested semantic type,
typed scopes and authority roles, bounded domain values, namespace-safe causal references, and
intrinsic author/scope/body/routing agreement without moving historical authorization into the
protocol layer. Added shared exact fixtures, both published-vector transitions, deep nested-value
checks, adversarial subject/scope/domain-bound/reference-alias cases, compile-fail trust-state
proofs, and semantic fuzz seeds. Format, architecture/behavior/causal/protocol verifiers, workspace
check/build/test/doctests, strict Clippy, dependency policy, four protocol targets, fuzz smoke,
whitespace, and unchanged Go build/vet/fresh full regression gates pass.

### Original plan entry

- **[protocol/high] Convert verified v1 DTOs into semantic facts** — Implement typed scope and
  causal-reference conversion, all family-specific intrinsic agreement checks, and the lossless
  transition from every verified canonical/control v1 DTO to its `SemanticFact` family. Add
  exhaustive bidirectional semantic fixtures, authority/scope/reference adversarial matrices, and
  conversion fuzz/property coverage. Complete this split package when all 48 semantic mappings are
  executable and no invalid or unsupported record can reach reduction as a falsely verified fact.

  **Implementation plan**

  - Add failing public API tests first that drive the two published vectors and one valid DTO for
    every family through `VerifiedSupportedRecord` into a new reducer-ready owner. Assert exact
    `FactKind`, event-ID identity, author key/address, authored milliseconds, scope, causal parents,
    typed authority roles, representative nested fields, and retained event/content evidence.
  - Introduce `VerifiedSemanticFact` as the sole successful conversion result. It owns the validated
    domain `SemanticFact` together with its prior verified DTO/event evidence, exposes immutable
    audit bytes and a fact borrow, and has no constructor from raw, parsed, prefix-supported, failed,
    or verified-unsupported values.
  - Implement small total primitive converters from fixed DTO types into every opaque domain ID/key,
    nonempty bounded text, provider/session, operation correlation, locator scheme/value, mailbox
    and installation address, timestamp, positive sequence, context, message, resource, binding,
    activity/runtime status, initial state, and remote result. Map validation failures to closed
    redacted semantic-conversion classes without carrying attacker text.
  - Convert signed scope and references before payload construction. Require exact protocol/scope
    isolation and common author agreement; erase canonical/control reference namespaces only after
    DTO direction checks, reject decoded-ID collisions, construct bounded sorted parent sets, map
    every closed authority role, require exact authority-parent linkage, and retain no wire string
    or generic JSON representation in domain state.
  - Implement one exhaustive numeric/body match producing all 48 `SemanticPayload` variants with no
    fallback or string inference. Preserve all optional values, ordered relay/resource arrays,
    correlations, project bindings, message provenance, runtime uncertainty, and remote command
    results exactly while applying the domain's narrower bounds such as error-code length.
  - Enforce family-specific intrinsic agreement at conversion: installation/creator/device/project
    roots versus verified author/key, family message purpose and output identity/project, local/peer/
    account/control audience and sender/source relations, peer-self exclusion, project primary
    membership/home, request target-home scope, receipt/outcome home signer, and every catalog
    body/envelope equality that requires no historical parent lookup. Leave ancestry, referenced
    family/subject, active authority, and reducer-state sufficiency to reduction.
  - Reuse a single shared integration fixture catalog so every exact DTO body is converted and its
    resulting payload kind is bidirectionally checked against `FactKind::ALL`; add deep equality
    checks for published family 1/46 mappings and focused representatives of every nested type.
    Add adversarial scope/author/body/route/reference/domain-bound cases and compile-fail trust-state
    examples proving unsupported and prefix-only types cannot expose a semantic fact.
  - Extend the structure-aware DTO fuzz target through semantic conversion and seed intrinsic-edge
    inputs. Run the pinned fuzz smoke plus format, all spec/architecture verifiers, workspace check/
    build/test/doctests, strict Clippy, root/fuzz dependency policy, four-target protocol checks,
    whitespace, and unchanged Go build/vet/fresh regression suite before recording.

  **Risks and mitigations**

  - Erasing a control/canonical namespace too early can alias causal references; validate direction
    first and reject any decoded-ID collision before constructing the domain's namespace-free parent
    set or authority map.
  - Wire short text is sometimes wider than its semantic destination, especially `ErrorCode`; run
    every domain constructor and return a stable `domain-value-invalid` class instead of truncating,
    normalizing, or retaining an invalid DTO as a fact.
  - Intrinsic checks can accidentally duplicate historical authorization policy; restrict protocol
    conversion to equalities available in the current signed envelope/body and leave parent family,
    subject, ancestry, frontier, active-membership, and aggregate decisions to pure reducers.
  - Forty-eight large match arms can silently swap same-width IDs; centralize primitive converters,
    name every body field explicitly, and pair the exhaustive kind table with exact deep checks for
    nested/address/provenance-heavy families.
  - Dropping wire evidence after semantic conversion would weaken replay/audit guarantees; retain
    the verified DTO owner inside `VerifiedSemanticFact` and expose exact event/content bytes without
    serializing domain structs back into the protocol.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including all 48 mappings, intrinsic and
     domain-bound adversarial matrices, fuzz smoke, and every repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so conversion code, tests, fuzz assets, and plan bookkeeping form one
     reviewable change.
