# Provider-neutral harness conformance v1

Status: Required adapter evidence

`hq-testkit::run_harness_conformance` is the reusable behavioral proof for the contract in
`docs/harness-contract-v1.md`. An adapter supplies a fresh `HarnessConformanceFixture` for every
scenario. Fixtures expose passive provider identity and capabilities, a factory under test, and a
read-only trace of neutral observations. Tests assert typed calls and values; diagnostic prose is
never parsed.

## Fixture rules

- Every scenario starts with a fresh provider factory and independent trace.
- The factory creates independently failing logical instances.
- Stable identities and digests in observations are the exact values received by the adapter.
- Event polling is deterministic and bounded; the suite does not depend on wall-clock sleeps.
- A conforming adapter may add private instrumentation, but the neutral trace contains no provider
  protocol or process detail.

## Required scenarios

| Scenario | Required proof |
| --- | --- |
| `UnsafeRegistration` | A declaration with neither stable idempotency nor authoritative lookup is rejected. |
| `NewSession` | Start returns only the exact acknowledged durable identity. |
| `ResumedSession` | Resume returns the requested existing identity unchanged. |
| `MissingResume` | A missing identity fails and no replacement session becomes ready. |
| `MismatchedResume` | A different acknowledgement is rejected and its owner is force-stopped once. |
| `ResponseLossAccepted` | Acceptance followed by response loss is found by exact identity-and-digest lookup and is not retried. |
| `ResponseLossMissingRetry` | Definite absence permits one retry with the identical identity and digest. |
| `ActiveOperationRace` | An intervening activity change does not bypass lookup-before-retry. |
| `ChangedInputCollision` | Reusing an identity with another digest fails closed. |
| `InteractiveRequest` | Structured requests are source-ordered, answered at most once, and released by operation cancellation. |
| `SecretRequestRejection` | A secret-bearing request fails closed without exposing its content in the neutral trace. |
| `OutputActivityOrder` | Typed bounded output and activity retain exact provider source order and correlation. |
| `CrashIsolation` | A crash in one logical instance does not corrupt a sibling instance or session. |
| `Teardown` | Intake closure, bounded drain reporting, and repeated force-stop leave no accepted work and perform one stop. |

`HarnessConformanceScenario::ALL` is the normative deterministic scenario inventory. The top-level
adapter test MUST compare its report with the entire inventory so adding a scenario cannot silently
leave an adapter untested.

## Scripted reference adapter

`ScriptedHarnessSubject` is an in-memory reference used to prove the suite itself. It deliberately
injects accepted-then-lost responses, missing-then-retry responses, an active-operation race,
identity collisions, structured and secret request paths, one-instance crash behavior, and pending
teardown work. It is test support only and is not a production adapter or supervisor model.

## Adapter completion evidence

An adapter is conformant when its own subject passes the complete reusable suite and its advertised
capabilities match the exercised behavior. Provider-specific protocol fixtures, process tests, and
installed-provider smoke tests remain additional adapter evidence; they cannot replace this neutral
suite.
