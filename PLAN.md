# HQ

## Next Up

### Preserve known-queued project delivery outcomes

The harness ledger distinguishes pending delivery from uncertain provider acceptance,
but the project runtime adapter currently reports both as uncertainty. Preserve that
closed distinction through the runtime port, durable saga, local API, CLI and TUI.

- Add a scoped delivery outcome for accepted, known queued, acceptance unknown and
  rejected delivery. Only exact ledger evidence may establish any disposition; keep
  project/assignment/thread/submission/digest correlation and acceptance-before-resume
  reconciliation. A missing observation is not proof of either queuing or acceptance.
- Persist a nonterminal queued saga disposition distinct from reconcilable uncertainty,
  retain its exact operation and submission identity, and return without spinning on
  immediate retries. Recovery scans and same-command replay must retain queued work;
  later accepted evidence commits one canonical dispatch. Preserve monotonic state and
  reservation rules through storage and reopen.
- Carry queued state through application/local API and clients. Show saved/waiting
  feedback without calling it acceptance uncertainty, success, or a rejection. Preserve
  unknown-acceptance reconciliation and failure semantics. Trace attempts, queued
  evidence and proven acceptance distinctly, using body-free correlation fields.
- Test pending versus uncertain runtime records, workflow queue/replay/acceptance,
  persistence/reopen and transitions, API conversion and client rendering. Preserve
  close/reassignment fences and exact-delivery deduplication; update affected docs.

Dependencies: exact project-session readiness already implemented. This is the delivery
prerequisite for the following runtime-recovery task; live readiness observations,
persisted bounded retries, automatic wake scheduling and runtime recovery controls remain
in that task. Complete when queued evidence remains distinct end to end and replay
consumes one canonical input only after proven provider acceptance.

### Expose runtime recovery status and schedule bounded retries

**Problem.** Durable assignment eligibility and device connectivity do not describe current agent
availability. Known-queued delivery is currently collapsed into acceptance uncertainty, and failed
recovery depends on later wake events rather than a bounded retry policy.

**Dependencies.** Exact project-session readiness before dispatch is implemented in
`0fe3b96`. Extend that behavior across the project workflow, node event scheduling,
application/local API types and TUI together.

**Scope and invariants.**
- Expose stopped, starting, ready, working and blocked observations correlated with exact agent,
  assignment, provider/session and node-generation/worker-owner identities. Old readiness results
  must not establish current liveness; device connectivity remains a separate status.
- Preserve known-queued versus acceptance-unknown state through a scoped runtime outcome, workflow
  and client. Reconcile uncertain provider acceptance before retry; preserve submission identity,
  input sequence and durable messages.
- Coalesce recovery for an assignment/session and persist bounded transient retries with backoff
  across restart. Trigger on pending work at startup and process loss with outstanding work. Avoid
  recurring whole-state scans. An unexpired prior-owner lease must schedule a later attempt without
  bypassing ownership. Preserve typed failure reasons rather than collapsing every resume error into
  unavailable. Permanent failure/exhaustion exposes typed Retry and View details.
- Revalidate and fence against close, reassignment, retirement and ownership changes. Idle stopped
  assignments need not launch, and missing saved sessions never become fresh conversations silently.
- Render actionable progress such as “Alice is restarting · Your message is saved.” Publish
  body-free invalidations; exact runtime/recovery evidence belongs in details. Distinguish submission
  attempts, proven acceptance and uncertainty in traces, including recovery paths.

**Completion.** A full client journey assigns Alice, receives a reply, restarts HQ and sends another
message; the same conversation resumes and delivers once with truthful status. Cover pending work
before startup, process loss, permanent/transient resume failure, concurrent sends, close/reassignment
races, stale queued work, retry exhaustion/restart and lost acceptance responses. Update
`docs/projects.md` and `docs/harness-supervisor-v1.md` to describe targeted retries and observations.
