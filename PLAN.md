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

- **[node/high] Drive bounded Unix sessions and coordinated graceful runtime** — Add the asynchronous
  accept/session/write loops over the owned listener, bounded connection/task/write capacity,
  incremental frame decoding, exact `ServerSession` write confirmations, coalesced invalidations,
  slow/nonreading-client containment, and lifecycle control routed through the sole node owner.
  Coordinate stop/restart requests and Unix signals through ordered component drain without lost
  acknowledgements or leaked clients. Test partial/multiple frames, malformed input, lost writes,
  saturation, slow readers, disconnect cleanup, signals, and connected-client stop/restart on Linux
  and macOS.

- **[node/high] Implement convergent autostart and lifecycle CLI roles** — Add one client coordinator
  that probes the owned socket, starts the foreground-node child only when absent, waits on typed
  readiness, and converges concurrent launchers on one owner without PID-file authority. Wire
  explicit foreground run, status, readiness, stop, and restart roles through the single `hq`
  executable with actionable phase/path/cause/action diagnostics. Test absent/stale/live nodes,
  concurrent starters, child failure, readiness timeout, lost lifecycle acknowledgements,
  connected-client reconnect after restart, and runtime artifact cleanup on Linux and macOS.

- **[transport/high] Specify and implement the encrypted Nostr envelope** — Write Nostr envelope v1
  independently from canonical v1, then implement recipient binding, NIP-44 encryption, NIP-59
  wrapping, NIP-42 authentication inputs, identity agreement, randomized transport timestamps,
  exact durable wrapper creation before first publish, and exact-byte reuse within a retry lineage.
  Define relay-visible data and input/quarantine bounds. Add standard vectors and tamper,
  wrong-recipient, signer mismatch, key reuse, retry, and size tests. Complete this work when opened
  envelopes yield only raw canonical bytes for the common verification/ingest path and transport
  metadata cannot grant domain authority.

- **[transport/high] Implement durable relay synchronization and replica convergence** — Implement
  one owner per relay session, retained catch-up with overlapping pagination, live subscription,
  NIP-42 authentication, outbound attempts, positive/negative acceptance, backoff, staging,
  quarantine, wrapper/logical deduplication, configuration refresh, and coalesced work wakes that do
  not restart healthy sessions. Use a deterministic scripted relay for disconnect, duplicate,
  response-loss, EOSE, offline catch-up, auth, and restart cases, followed by a controlled real-relay
  smoke test. Complete this work when two distinct Rust installations converge across arbitrary
  delivery order and downtime without relay observations influencing reduction.

- **[harness/high] Define the provider-neutral harness contract and conformance suite** — Specify
  logical instances, durable sessions, capabilities, start/resume readiness, stable submission IDs,
  accepted/rejected/uncertain outcomes, lookup/reconciliation requirements, interactive requests,
  normalized output/activity, cancellation, and shutdown. Implement neutral traits and a scripted
  fake provider; registration must reject adapters lacking safe idempotency or reconciliation.
  Ensure neutral crates contain no Codex vocabulary. Complete this work when the fake passes a
  reusable conformance suite covering new/resumed sessions, response loss, active-operation races,
  interactive requests, output, crash isolation, and teardown.

- **[harness/high] Implement supervisor ownership, delivery recovery, and bounded persistence** —
  Implement one logical worker owner per named agent, durable ownership and delivery ledgers,
  pending/uncertain/accepted reconciliation, automatic wake from durable pending work, bounded FIFO
  plus keyed coalescing, output-before-activity persistence, stable output collision checks,
  environment-copy/redaction policy, and stop-intake/drain/escalate shutdown. Test daemon restart,
  lease races, response loss, buffer saturation, coalescing order, partial output/activity commits,
  concurrent agents, secret exclusion, drain timeout, and forced process termination with the fake
  adapter. Complete this work when accepted work is never silently lost or duplicated.

- **[codex/high] Implement and pin the Codex provider adapter** — Select a current supported Codex
  app-server baseline using official schema/documentation and installed-binary evidence, pin its
  generated fixtures, and privately implement process startup, bounded JSONL/JSON-RPC transport,
  initialization, exact thread start/resume/read behavior, turn start/steer/interrupt,
  stable-submission reconciliation, supported server requests, additive notification tolerance,
  normalized output/activity, typed failure causes, stderr trust boundary, and shutdown escalation.
  Keep every Codex DTO and method name out of neutral crates. Complete this work when the neutral
  conformance suite, pinned protocol fixtures, process tests, and opt-in installed-provider smoke
  test pass.

- **[resources/high] Implement path-resource identity, conflict, health, and release assessment** —
  Implement home-qualified absolute path locators, human spelling versus canonical identity,
  nearest-existing-ancestor handling for missing paths, symlink revalidation, equal/ancestor/
  descendant conflict detection, project-local overlap, resource health, Git cleanliness, primary
  path selection, launch-directory validation, and advisory claim persistence. Keep filesystem/Git
  observations outside pure project policy and never silently relocate or delete resources. Test
  missing/inaccessible paths, symlinks, worktrees sharing a Git directory, dirty/unknown release,
  atomic replacement, and explicit force behavior. Complete this work when every path decision is
  deterministic, explainable, and auditable.

- **[projects/high] Implement project command, activation, dispatch, and provisioning sagas** —
  Implement home-authoritative project commands/results, durable remote routing, expected-head
  serialization, resource acquisition/release, assignment/configuration/runnable transitions,
  thread creation/resume, pending-message sequencing and at-most-once dispatch, graceful and forced
  close/takeover, retirement, late output, and Git worktree provisioning. Every filesystem, Git,
  network, and provider boundary must use a stable operation ID, explicit checkpoint,
  reconciliation-before-retry, compensation, and definite-versus-unknown outcome. Test crashes and
  failures at every boundary, stale commands, competing devices, blocked handoff, restart repair,
  and complete attribution. Complete this work when every saga reaches a documented stable or
  explicitly reconcilable state.

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
