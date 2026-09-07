# HQ

## Next Up

### Expose exact harness readiness and lease evidence

Runtime recovery needs current worker ownership and retained lease deadlines. Exact resume
currently returns only a session ID, and lease discovery requires a bounded aggregate
snapshot that can omit the desired agent.

- Return typed readiness evidence from exact resume containing the actual live agent,
  project, provider/session and worker owner. Construct it under the worker lock after
  successful ownership renewal or launch; durable ready-session receipts alone never
  establish a live worker. Keep exact-resume identity checks and no fresh fallback.
- Add an exact agent-keyed lease read through harness state, store actor/database and
  node adapter. Preserve the retained owner and absolute deadline, including expired
  rows; absence is not a readiness claim. Use indexed exact reads, not snapshot scanning.
- Test concurrent exact resume reuses one owner, stop/restart changes ownership, retained
  sessions without workers require resume, mismatched identity and lost ownership fail
  closed, and unexpired foreign leases remain available as deadline evidence without
  launching. Verify exact lease reads, release and reopen through real storage.
- Update supervisor documentation and consumers for the typed return value. Preserve
  delivery eligibility, project queue exclusion, exact acceptance and existing fences.

This supplies evidence for the following runtime-recovery task. Assignment/node-generation
projection, failure classification across application boundaries, persisted bounded retry
scheduling and user controls remain there. Complete when runtime readiness names a current
worker owner and lease deadlines can be queried by exact agent without a global scan.

### Expose runtime recovery status and schedule bounded retries

**Problem.** Durable assignment eligibility and device connectivity do not describe current agent
availability. Failed recovery depends on later wake events rather than a bounded retry policy.

**Dependencies.** Exact project-session readiness before dispatch is implemented in
`0fe3b96`; exact queued versus acceptance-unknown delivery evidence is preserved end to end in
`f53aff7`. Extend these behaviors across the project workflow, node event scheduling,
application/local API types and TUI together.

**Scope and invariants.**
- Expose stopped, starting, ready, working and blocked observations correlated with exact agent,
  assignment, provider/session and node-generation/worker-owner identities. Old readiness results
  must not establish current liveness; device connectivity remains a separate status.
- Integrate the existing known-queued versus acceptance-unknown outcomes with recovery status
  and retry scheduling. Reconcile uncertain provider acceptance before retry; preserve submission
  identity, input sequence and durable messages.
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
