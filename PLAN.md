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

- **[release/high] Complete the Rust cutover package and evidence audit** — Update supported-install
  and operator documentation, service-manager guidance, recovery boundaries, and the release
  changelog/version candidate. Rehearse rollback to an untouched archived synthetic Go
  installation without starting it or opening its state, produce the cutover checklist and
  evidence bundle, and audit every acceptance-matrix row and definition-of-done clause. Complete
  this work when an operator can separately authorize soak and cutover with known rollback steps;
  do not tag, publish, replace, disable, or activate any live installation.
