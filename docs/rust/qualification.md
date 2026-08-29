# Rust system qualification

Status: normative acceptance and performance evidence contract

This document turns the acceptance matrix in `rust-rewrite-design.md` into reproducible evidence.
The machine-checked inventory is `qualification/acceptance-evidence.tsv`; the quantitative source
of truth is `qualification/budgets.env`. Documentation, a successful compile, or a cross-target
check is never substituted for a missing behavioral test.

## Acceptance evidence

`scripts/verify-rust-qualification.sh` rejects an unknown, missing, or empty acceptance area,
untracked or unresolved evidence, a renamed or malformed Rust test selector, a non-executable
command, an unknown proof kind, and duplicate evidence. `--validate-only` performs those checks
without compiling or running the workloads. The inventory covers exactly these rows:

| Acceptance area | Direct evidence focus |
| --- | --- |
| Domain/algebra | Nine laws, generated DAG schedules, conflicts, and maximal frontiers |
| Canonical protocol | Exact v1 vectors, strict/adversarial inputs, signature and size boundaries |
| Authorization | Capability and account grant/revoke/regrant/observe races |
| Persistence | Mutation failpoints, repair equality, incremental/batch equality, crash/reopen |
| Queries | Canonical order, stable cursors, and indexed later-page behavior |
| Local API/node | Replay, reconnect, revision races, readiness, restart, and shutdown |
| Relay | Wrappers, retry, duplicates, authentication, disconnect, catch-up, and wake behavior |
| Harness | Capability validation, uncertainty, ordering, backpressure, drain, and kill |
| Projects | Transition model, expected-head races, conflicts, and saga compensation |
| TUI/CLI | Retained workflows, pure transitions, rendering, and terminal restoration |
| Security/operations | Redaction, bounded resources, architecture, recovery, and budgets |

Every behavioral inventory row names one exact `test:function_name`; command and configuration
rows use closed proof kinds. The complete workspace suite remains the proof that the named tests
pass together; the inventory is checked traceability, not a second hand-maintained test list.

## Integrated gap audit

The native Apple-Silicon audit on 2026-08-29 resolved every acceptance-matrix requirement to a
current exact test, command, or configuration proof. It found no unexplained behavioral failure,
untested in-process invariant, or exceeded budget. In particular, direct evidence exists for:

- generated algebra and authorization schedules, strict protocol vectors, and fuzz boundaries;
- atomic failpoints, corrupt-projection recovery, indexed cursors, and incremental/batch equality;
- local replay/reconnect/restart, relay outage and response loss, and harness partial persistence;
- project expected-head races, compensation, and bounded recovery scans;
- pure TUI workflows, responsive rendering, installed terminal restoration, and CLI repair; and
- secret/environment redaction, bounded queues and tasks, and every quantitative resource gate.

Evidence that inherently depends on another native target or an operator-controlled external
system remains explicit work, not a waived gap. Native Linux/macOS target records belong to the
next qualification queue item. Controlled relay/provider smoke, backup/restore, catch-up, node
replacement, and read-only Go archival rehearsals belong to the queued release-candidate item and
retain their separate authority boundaries.

## Performance workloads and budgets

These are regression gates, not claims about best-case latency. Wall-clock ceilings are deliberately
wide enough for shared supported CI runners while still rejecting order-of-magnitude regressions.
Semantic and boundedness assertions must pass even when timing is below the ceiling.

| Gate | Representative workload | Maximum |
| --- | --- | ---: |
| Cold readiness | Initialized identity to a foreground node publishing authenticated readiness | 5,000 ms |
| Full rebuild | Reverify and atomically rebuild 1,002 mixed conversation facts | 10,000 ms |
| Late-parent/high-fanout ingest | Insert one missing authority parent waking 500 durable dependants | 5,000 ms |
| Long-conversation paging | Load ten indexed later pages from a 1,000-entry conversation | 1,000 ms |
| Invalidation-to-redraw | Apply an invalidation to a ready model holding 10,000 stable rows | 100 ms |
| Bounded queue behavior | Drain saturated fixed-capacity local-session queues | 1,000 ms |
| Idle resident memory | Ready foreground node before client pressure | 128 MiB |
| Active resident memory | Ready node with admitted local connections and repeated status work | 192 MiB |
| Release build time | Clean locked optimized build of the single `hq` executable | 900 s |
| Graceful shutdown | Stop acknowledgement through process exit and artifact cleanup | 5,000 ms |

The environment file uses fully descriptive variable names and decimal milliseconds, seconds, or
kibibytes. The runner exports those values to the owning Rust tests. Running a test directly uses
the same checked-in fallback and is useful for development; release evidence comes from the runner.
Qualification timing workloads serialize through a test-local lease, and the runner also fixes the
test-thread count to one so unrelated scheduler contention is not mistaken for an algorithmic
regression.

## Platform evidence

Run:

```sh
scripts/verify-rust-qualification.sh
```

The command validates the inventory, runs the owning budget suites, performs an isolated clean
release build, and prints a platform record containing the operating system, architecture, Rust
host, git revision, build duration, and all budget values. It does not alter the ordinary target
directory.

Native runtime evidence is required on Linux and macOS. ADR-0001 additionally requires portable
crate compilation for x86-64 and ARM64 on both systems. Cross-compilation proves only that the
portable code compiles for that target; it does not prove kernel credentials, Unix-socket lifecycle,
terminal behavior, installed providers, resident memory, or process shutdown on that target.

Any missing row, unexplained failure, untested invariant, or exceeded budget is new work for
`PLAN.md`. It is not waived by raising a limit during the same qualification run.
