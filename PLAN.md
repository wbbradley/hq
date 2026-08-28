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

- **[cli/high] Export and join offline-verifiable human pairing invitations** — Add bounded signed
  invite export and guarded join through pure application plans and the canonical protocol. An
  invitation carries the complete account creator/grant/regrant authority needed for offline target
  verification plus exact target installation/key and bounded relay hints; it contains no root
  secret or local operational state. Join verifies canonical bytes, signatures, target binding,
  lineage, expiry policy if specified, and changed reuse before accepting and selecting membership.
  Test tampering, wrong target/key/account, missing history, duplicate replay, concurrent revoke,
  restart, unsafe paths, and deterministic human/JSON rendering.

- **[cli/high] Inspect and revoke human account devices** — Add typed device listing and
  creator-only revoke through authoritative snapshots and pure application plans. Preserve every
  maximal acceptance/revoke, require exact grant attribution, fan revocation out to the named device
  before route blocking, and expose pending/active/revoked/conflicted or incomplete states without a
  chosen historical winner. Test non-creator rejection, stale/incomplete frontiers, concurrent
  acceptance/revoke, regrant ancestry, response loss, restart, fanout, and human/JSON rendering.

- **[cli/high] Implement directional peers and mailbox capabilities** — Add peer add/list/distrust
  and mailbox grant/revoke/inspection commands over exact application plans and authoritative
  snapshots. Keep route trust directional and distinct from mailbox authority; preserve historical
  observations, revoke-before-block delivery ordering, full installation-qualified addresses, and
  fail-closed concurrent/later authorization. Test stale frontiers, replay, block recovery, relay
  hints as non-authority, and local-API-only architecture.

- **[cli/high] Implement relay policy, synchronization, health, and repair administration** — Add
  relay add/list/remove, explicit sync, domain/delivery health status, and explicit repair commands
  over typed local effects and authoritative observations. Preserve stable effect identities,
  accepted/rejected/uncertain reconciliation, bounded relay policy, offline queues, prompt-wake
  semantics, and repair as an explicit audited operation. Test response loss, restart, disabled and
  incompatible relays, stale revisions, offline rendering, redaction, and end-to-end fake-node
  coverage.

- **[cli/high] Implement mailbox messaging and repository-aware discovery** — Add `ask`, `send`,
  `wait`, `poll`, `get`, human list/filter, answer, cancel/archive, restore, and repository-aware
  mailbox discovery over typed application/local API operations. Preserve stable message identity,
  causal reply/cancellation authority, non-consuming inspection, duplicate-safe ready delivery,
  asynchronous send, and intentionally unbounded human wait with bounded per-attempt I/O. Support
  explicit session mailbox selection without ambiguous provider inference. Test restart, reconnect,
  incomplete history, duplicate delivery, stale targets, non-TTY input, filters, and machine output.

- **[cli/medium] Implement named-agent sessions and embedded agent guidance** — Add named-agent
  list/show/create/rename/retire and neutral start/exact-resume/stop workflows plus current-session
  discovery for supported provider environments. Ship concise installed guidance for messaging,
  retry, synchronization, delivery identity, causal incompleteness, and human-owned administrative
  boundaries. Test provider ambiguity, stale sessions, resume mismatch, runtime uncertainty,
  redacted diagnostics, generated help, and local API-only architecture.

- **[cli/high] Implement project/resource/worktree commands and audit non-TUI parity** — Expose
  project list/show/create/send/open/activate/handoff/close/archive/unarchive, desired-resource
  add/remove/replace/check, remote-command progress, and recoverable worktree provisioning through
  the existing project and inspection ports. Render checkpoints, conflicts, runtime uncertainty,
  claims, and orphaned external-state warnings explicitly. Add parser, response-loss, restart,
  stale-head, force-confirmation, fake-node, and real foreground end-to-end tests, then audit every
  retained non-TUI behavior-ledger workflow and close any remaining CLI gap.

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
