# HQ

## Next Up

### Composer approval handoff

Use the conversation's lower surface for pending command approval, replacing the
editor without a modal or third Tab stop. While composing, show “Approval needed”
near the editor bottom in the alert semantic color without interrupting typing.
Tab preserves draft/cursor and switches directly to approval UI; Tab/Shift-Tab then
cycle transcript and approval focus. Resolution restores the focused composer with
its draft and cursor, preserving transcript position and tail state throughout.
Keep exact typed approval correlation and prevent editing keystrokes from approving.

Touch TUI approval/draft/focus state, lower-surface rendering, footer hints and
conversation docs. Dependencies: persistent conversation composition. Complete when
model, render, and installed terminal tests prove approval arrival while typing,
explicit handoff, both focus directions, stale/mismatched approval handling, and
exact draft/cursor restoration after resolution. The pending-approval alert is
contextual status, not a duplicate ordinary composer-shortcut row.

### Show composer shortcuts once in the global footer

Conversation composition currently displays duplicate shortcut rows: `render_draft_pane`
always reserves a local hint row, while `render_footer` also advertises send, newline,
and close actions.

- In `crates/hq-tui/src/render.rs`, remove the normal shortcut row from
  `render_draft_pane`. Let draft text use the entire inner area when no
  `message_field_error()` exists; reserve a row for error-styled validation feedback
  only while an error exists.
- Keep composer shortcut guidance in the global footer. Cover conversation drafts
  and standalone drafts: standalone composition uses
  `UiRoute::Form { capability: StartNewWork, .. }`, whose generic form footer currently
  takes precedence over the draft-focus branch. Select composer guidance for that
  typed composition state without changing unrelated route or notification behavior.
- Extend `crates/hq-tui/tests/render_snapshots.rs` to cover conversation and standalone
  composition at normal and narrow sizes. Assert shortcut guidance occurs once in
  the footer, the reclaimed row belongs to the text editor, and validation feedback
  remains visible and releases its row when cleared. Preserve recipient/project
  context, save status, byte count, and caret rendering.

Dependencies: none.

Complete when ordinary composition has one shortcut footer, valid drafts no longer
reserve a redundant hint row, and invalid drafts retain visible inline validation
feedback.

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
