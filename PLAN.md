# HQ Rust Rewrite

## Goal

Build a clean-sheet Rust implementation of HQ that satisfies
`rust-rewrite-design.md`, the reviewed Rust-era specifications produced by this plan, and the
acceptance matrix in that design. Preserve the distributed causal-fact model and its algebraic
laws. Do not preserve Go databases, canonical schemas, local RPC, command syntax, package
boundaries, UI details, or other compatibility merely because the Go implementation has them.

The autonomous work ends with a verified, cutover-ready Rust release candidate and a rehearsed
operator procedure. Replacing, deleting, or disabling a live Go installation, activating a
production identity, and declaring a production soak successful require separate user authority.

## Authoritative sources

Use these sources in order when they disagree:

1. The decisions, non-goals, invariants, acceptance matrix, and definition of done in
   `rust-rewrite-design.md`.
2. Rust-era specifications and ADRs created by this plan.
3. The algebraic laws currently recorded in `../crdt-algebra-laws.html` and then copied into the
   tracked Rust-era specifications.
4. Retained product intent extracted from `docs/`, `rust-port.md`, and
   `rust-port-transcript.md`.
5. Frozen Go code and tests as scenario sources and defect evidence, never as a compatibility
   oracle.

## Execution contract

- The ordered list under **Next Up** is the complete remaining roadmap, not merely the next task.
- Execute the first item with the `next-task` skill using the exact path
  `/Users/wbbradley/src/hq/PLAN.md`. After completing and recording it, continue with the new first
  item until the queue is empty.
- If the first item is too large for one coherent, reviewable commit, split it into smaller
  capability-named items at the front of the queue before implementation. Do not encode sequence
  positions in durable names.
- Refine later items as specifications and implementation reveal better boundaries. Add every
  newly discovered requirement, regression, recovery obligation, or acceptance gap to the queue at
  the correct dependency position. Never narrow or delete required scope except by completing it
  and recording it in `COMPLETED.md`.
- Use test-first development where practical. Keep domain policy pure, dependencies pointed inward,
  and I/O behind the boundaries in the rewrite design. Every task must leave the repository
  formatted, lint-clean, tested in proportion to its risk, and committed with a Conventional
  Commit.
- Resolve routine implementation choices autonomously. Choose the simplest first-principles option
  consistent with the architecture, security model, and retained behavior, and record material
  decisions as ADRs. Stop only for a decision that changes product scope or security posture,
  expands authority, requires destructive or externally consequential action, or remains genuinely
  irresolvable from evidence.
- Do not claim the goal complete merely because **Next Up** is empty. Audit every acceptance-matrix
  row and the definition of done in `rust-rewrite-design.md` against current evidence; add any
  missing work back to the queue and continue.

## Next Up

- **[projects/high] Implement activation and at-most-once project dispatch** — Add the
  transaction-consistent canonical project mutation capability and explicit activation workflow:
  expected-head/home/active-human validation, resource observation and claim preview, conditional
  open, configuring assignment, project-bound start or exact resume, launch-directory validation,
  thread selection from the first pending project message or explicit historical resume, runnable
  transition, and compensation to the documented prior stable state. Drain accepted inputs in home
  sequence through the harness supervisor's sole durable delivery ledger, reconcile before retry,
  and author dispatch only after definite acceptance. Test every crash and definite/unknown failure
  boundary, stale heads, claim/agent conflicts, launch failure, pending-message preservation,
  accepted-response loss, changed input, restart repair, late output, and complete attribution.

- **[projects/high] Implement project lifecycle, resource, handoff, and retirement workflows** —
  Implement open/archive, resource add/remove/replace, release assessment, graceful and forced
  close, graceful handoff and forced takeover, and retirement over explicit durable checkpoints.
  Dirty or unknown resources require force before releasing authority; graceful operations retain
  claims until quiescence, failed handoff becomes blocked, forced actions revoke only HQ authority,
  and retirement ends assignment before retiring the agent while the project stays open. Test
  definite/unknown runtime and filesystem outcomes, compensation, competing agents/devices, stale
  commands, blocked handoff, restart recovery, and no implicit resource mutation or deletion.

- **[projects/high] Implement durable remote project command routing and local API progress** —
  Extend `hq-local-api` with the typed project request/outcome and authoritative checkpoint view.
  Non-home devices author only strict `RemoteProjectCommandRequested` facts; the immutable home
  derives typed receipt parents from one serialized snapshot, executes the same workflow, and
  authors exactly one committed, rejected, or explicitly uncertain result. Validate digest/body
  agreement and expected heads, reject unknown codec versions, and expose queued/received/terminal
  progress without reducer side effects. Test offline routing, competing devices, duplicate and
  changed command identities, stale receipt/result, restart repair, and complete control-plane
  attribution.

- **[projects/high] Implement recoverable Git worktree provisioning and compose project workers** —
  Add a separate bounded mutating Git capability with stable lookup/create operations, short-lived
  repository serialization, destination reservation, exact worktree/branch reconciliation,
  read-only `hq-resources` identification, and one canonical project creation. Resume after every
  reservation, Git, identification, and canonical boundary without duplicate worktree/project;
  never silently delete external state on uncertainty. Compose project workflow, store, harness,
  resources, Git, canonical mutation, wake/recovery, intake, and shutdown ownership in `hq-node`.
  Run bounded startup scans, checkpoint all accepted work before harness/store shutdown, add
  model/failpoint tests for every boundary and reservation conflict, and finish project,
  application/local API, storage, behavior-ledger, acceptance, architecture, and four-target CI
  evidence.

- **[cli/medium] Complete the Rust command-line client** — Implement the retained command workflows
  from the behavior ledger over `hq-local-api` only, including identity/account, messaging, peers,
  relays/status/sync, named agents and sessions, projects/resources/worktrees, lifecycle control,
  noninteractive output, machine-readable output, and actionable typed diagnostics. Do not preserve
  Go spelling where a simpler coherent Rust-era surface is better. Add parser, rendering, retry,
  autostart, non-TTY, and end-to-end fake-node tests plus generated help. Complete this work when
  every retained non-TUI workflow is available without direct storage, signing, relay, or provider
  access.

- **[tui/high] Build the pure Ratatui application architecture and terminal shell** — Implement
  `UiModel`, the closed `UiEvent` enum, pure update transitions, explicit `UiEffect` values, stale
  effect-response suppression, one effect executor, borrowed rendering, responsive layout,
  terminal input, redraw scheduling, reconnect state, and RAII terminal restoration. Add model and
  effect tests, deterministic buffer snapshots across representative terminal sizes, and normal,
  error, cancellation, and panic restoration tests. Complete this work when the UI shell has no
  domain/storage side channel and can render/reconnect against a scripted local client.

- **[tui/high] Implement retained mailbox, agent, and project workflows** — Add authoritative
  snapshot reload, conversation/activity presentation in reducer order, inbox filtering, typed
  technical disclosure, reply/new-message composition, durable drafts, focus, logical selection and
  scroll anchors, archive/restore, agent/session management, project-first composition, resource
  conflict previews, activation/close/takeover flows, progress, and actionable errors. Preserve
  semantic user workflows rather than Bubble Tea cells or key bindings. Test invalidation and
  resize during editing, reconnect with in-flight commands, stale targets/heads, activity
  coalescing, modal cancellation, and complete CLI/TUI use-case parity where intended. Complete
  this work when every retained interactive workflow survives reload and never reimplements domain
  ordering or authority.

- **[verification/high] Qualify the integrated Rust system against the acceptance matrix** — Run and
  strengthen the complete fixture, property, model, fuzz, crash/reopen, lifecycle, architecture,
  security/redaction, and end-to-end suites across the assembled node, clients, relay, harness, and
  project workflows. Establish and meet explicit budgets for cold readiness, full rebuild,
  late-parent/high-fanout ingestion, long-conversation paging, invalidation-to-redraw, bounded queue
  behavior, memory, release build time, and graceful shutdown. Run platform evidence on the
  ADR-0001 Linux/macOS matrix. Audit every acceptance-matrix row against direct evidence and add
  missing work to the queue rather than waiving it. Complete this work only when unexplained
  failures, invariants without tests, and algorithmic regressions are absent.

- **[release/high] Produce and rehearse the cutover-ready Rust release candidate** — Build the
  single-executable Linux x86-64/ARM64 and macOS x86-64/Apple-Silicon release artifacts, complete
  operator and recovery documentation, verify identity backup behavior in
  first-release scope, and dogfood only with new identities and new state directories on controlled
  relays. Rehearse installation, startup, offline catch-up, relay loss, provider crash, database
  repair, backup/restore where supported, node replacement, clean shutdown, and rollback to an
  untouched archived Go installation. Produce a cutover checklist and evidence bundle without
  opening or mutating Go state and without switching a live production identity. Complete this work
  when the definition of done is proven and an operator can separately authorize soak and cutover
  with known rollback steps.
