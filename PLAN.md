# HQ

## Next Up

### Resume exact project sessions before dispatch

**Problem.** After restart the durable assignment remains runnable but the supervisor has no
worker. Project dispatch queues a message without resuming that worker. Ordinary project resume
also risks draining stale project deliveries without checking current assignment eligibility.

**Scope.** Add an idempotent exact-session readiness operation to the harness supervisor and project
runtime port. Carry the saved launch directory in canonical workflow snapshots, validate it before
readiness, and recheck current assignment/input eligibility after readiness before dispatch. Resume
must not drain project delivery records; the project workflow selects each eligible input explicitly.
Keep direct-session recovery behavior intact. Correct premature submission trace evidence.

**Dependencies and invariants.** Build on existing supervisor ownership, exact resume and submission
lookup. Preserve the project assignment, session, stable input identity and authoritative sequence.
Concurrent readiness calls reuse the matching live worker; mismatched project/provider/session must
fail closed. Failed resume never creates a new conversation or submits a message. Runtime readiness
is observed anew, never inferred from an old successful operation. No prerequisite task exists.

**Completion.** Regression tests cover restart with saved project input, repeated readiness without
a second worker, exact-session mismatch, stale project records left undrained, failed resume, launch
validation, eligibility changes during resume, and acceptance-response loss without resubmission.
Project dispatch automatically resumes the exact session and submits once; related runtime contract
docs describe the behavior. Persisted retries and the client runtime-state projection follow below.

### Expose runtime recovery status and schedule bounded retries

**Problem.** Durable assignment eligibility and device connectivity do not describe current agent
availability. Known-queued delivery is currently collapsed into acceptance uncertainty, and failed
recovery depends on later wake events rather than a bounded retry policy.

**Dependencies.** Requires exact project-session readiness before dispatch. Extend the project
workflow, node event scheduling, application/local API types and TUI together.

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
