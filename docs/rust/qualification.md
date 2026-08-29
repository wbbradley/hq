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

## Integrated acceptance audit

The four-host audit on 2026-08-29 resolved every acceptance-matrix requirement to a current exact
test, command, or configuration proof. It found no unexplained behavioral failure, untested
in-process invariant, or exceeded budget. In particular, direct evidence exists for:

- generated algebra and authorization schedules, strict protocol vectors, and fuzz boundaries;
- atomic failpoints, corrupt-projection recovery, indexed cursors, and incremental/batch equality;
- local replay/reconnect/restart, relay outage and response loss, and harness partial persistence;
- project expected-head races, compensation, and bounded recovery scans;
- pure TUI workflows, responsive rendering, installed terminal restoration, and CLI repair; and
- secret/environment redaction, bounded queues and tasks, and every quantitative resource gate.

Native Linux and macOS evidence is recorded below. The release workflow additionally rehearses a
pinned controlled relay, relay loss and catch-up, provider crash and drain, identity-only recovery,
database repair, node replacement, and offline selection of an untouched synthetic Go archive.
These controlled proofs retain their separate authority boundaries and never imply a production
soak or cutover.

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

Native runtime evidence is required on Linux and macOS. `qualification/platform-matrix.tsv` maps
the four required OS/architecture combinations to explicit native GitHub runner labels and Rust
hosts. `.github/workflows/rust-qualification.yml` runs the complete qualification command on every
row, uploads one immutable environment record per target, and aggregates the records only after
their schemas, Git revision, host identities, budgets, and exact target set agree. The aggregate
artifact and workflow summary are the durable evidence for a particular commit.
`scripts/test-rust-qualification-matrix.sh` exercises both acceptance and rejection paths for the
record-set validator without claiming simulated records as platform evidence.

ADR-0001 additionally requires portable crate compilation for x86-64 and ARM64 on both systems.
Cross-compilation proves only that the portable code compiles for that target; it does not prove
kernel credentials, Unix-socket lifecycle, terminal behavior, installed providers, resident memory,
or process shutdown on that target. Those claims therefore come only from the native matrix.

## Recorded native qualification

Implementation revision `762f0785059a87cf8c9bfeb34a6bd11bdc54de4a` passed the complete native
matrix in [GitHub Actions run 33250739592](https://github.com/wbbradley/hq/actions/runs/33250739592).
The combined artifact is
`rust-qualification-matrix-762f0785059a87cf8c9bfeb34a6bd11bdc54de4a`. Its four environment
records were downloaded together and independently revalidated with
`scripts/verify-rust-qualification-matrix.sh` against that exact revision:

| Runner | Operating system | Architecture | Rust host | Release build |
| --- | --- | --- | --- | ---: |
| `ubuntu-24.04` | Linux | x86_64 | `x86_64-unknown-linux-gnu` | 95 s |
| `ubuntu-24.04-arm` | Linux | aarch64 | `aarch64-unknown-linux-gnu` | 94 s |
| `macos-15-intel` | Darwin | x86_64 | `x86_64-apple-darwin` | 184 s |
| `macos-15` | Darwin | arm64 | `aarch64-apple-darwin` | 165 s |

Every record names the same full revision, the expected native host, the complete checked-in budget
set, and a clean release build below the 900-second limit. The aggregate validator also rejects a
missing, extra, malformed, host-mismatched, budget-mismatched, or different-revision record.

The final acceptance audit additionally reran the installed TUI terminal lifecycle target, the
provider-neutral harness conformance suite, the real Codex adapter seam, the architecture gate, the
evidence inventory validator, and the matrix validator's acceptance and rejection cases. The eleven
acceptance rows therefore have direct current behavioral or configuration evidence, all Rust-era
protocol and ownership boundaries remain architecture-checked, durable and external-effect
boundaries retain deterministic recovery evidence, and normal Rust operation has no Go code path,
protocol, or state dependency.

`qualification/cutover-evidence.tsv` is the closed audit of the same eleven acceptance rows plus
every definition-of-done clause: reviewed requirements and algebra, Rust-era protocol
specifications, durable/external recovery, Go-independent normal operation, causal authority,
convergence, atomicity, lifecycle ownership, and domain state transitions. The release aggregate
validates that inventory and emits `hq-rust-cutover-evidence-v1`, binding the native release,
recovery, controlled-failure, and rollback records by SHA-256. The record explicitly preserves two
unperformed operator decisions: soak authorization and separate cutover authorization.

The final completion audit passed in GitHub Actions run
[33264900059](https://github.com/wbbradley/hq/actions/runs/33264900059) for exact revision
`7317efae3aea99150c5d4d5eb3c729517fd11bb1`. The acceptance inventory now contains 70 direct proofs:
all nine algebra laws individually, every acceptance subclaim, representative retained installed
workflows, every quantitative budget, and every recovery drill. The cutover contract retains the
same exact 11 acceptance clauses and nine definition-of-done clauses while binding the recovery
clause separately to identity/database, relay/provider, project-saga, and archived-Go rollback
evidence. Protocol proof now checks every required Rust-era specification and its executable
consistency suites. Normal-operation proof separately rejects Go code, tooling, state, packaging,
and service inputs, including positive and tampered fixtures.

An independent fresh download passed all five validators and reproduced the release, recovery, and
cutover aggregates byte-for-byte. Their SHA-256 digests are respectively
`b71510aaa50ea743f924500b8e6c3026e4560eddd499a5edf41cd061dbe22d92`,
`4244542c918dec9490c216ed3d57334dea32a6cf61e241d702eec9fc5fc0c293`, and
`d40e56906b5a35d88b0e4b1398c4f9701c9d0b6c828a7d1167d1318d700c13a0`; the controlled-failure and
rollback evidence digests are `a21cb98a5bf826c4661a50f5dad4e99953b6d23e94fd1b4c561e014ff78776c5`
and `36c9a975a14d12087326fac5ba4840032ccf14aba51e71e1472637ee8234b08a`.
The unshipped clean-sheet database identifies itself as storage v1 and has no migration path, the
stored and runtime harness operation state is one shared type, passive records keep idiomatic public
fields, and the dependency audit contains no yanked cryptographic package. No acceptance or
definition-of-done gap remains.

Any missing row, unexplained failure, untested invariant, or exceeded budget is new work for
`PLAN.md`. It is not waived by raising a limit during the same qualification run.
