# HQ

## Next Up

### Conversation display preferences

Persist and expose the two settings required by conversation reading and persistent
composition: page overlap in rendered lines (default one) and focused composer
height cap as a percentage of available height (default one-third). Use the current
installation configuration, local API, and Config screen; validate numeric values
and preserve unrelated defaults on field-specific updates. Zero page overlap is
valid; allow only positive composer percentages below 100 so the transcript retains
space. Runtime geometry must still clamp overlap to permit paging and the composer
cap to keep both surfaces usable on small terminals.

Touch installation configuration encoding/decoding and mutation, local API types,
node/TUI configuration mapping, Config input/rendering, and configuration docs.
Complete when defaults, persisted round trips, invalid values, isolated field
updates, and editable Config controls are covered by tests. These settings are
consumed by the following reading and composition tasks; their behavior remains
unfinished until those tasks land.

### Conversation reading, inspection, and history paging

Ordinary conversation navigation must be a wrapped-line scroll position rather
than message selection. Reuse renderer-measured entry geometry and stable entry
identity plus wrapped-row position.

- In transcript reading, `j`/`k` and up/down scroll one rendered line, including
  wrapping and explicit line breaks. Remove ordinary message-selection highlights.
- PageUp/PageDown scroll a viewport using the persisted page-overlap preference;
  clamp overlap so small viewports still advance. Open at the latest history and
  load older history automatically near the top without moving the content being
  read. Prevent duplicate requests, retain explicit retry after failure, and stop
  at exhausted history. Home reaches oldest loaded content; End reaches the bottom
  and enables tail mode. Editor navigation retains editing meanings.
- Follow incoming content only while already at the tail. Otherwise preserve the
  prior reading position across append, streaming updates, history prepend, resize,
  and redraw. Show a new-content indicator and discoverable jump-to-latest action.
  Typing or switching focus must not independently enable tail mode.
- Provide explicit Inspect mode, provisionally activated by Enter. Highlight a
  visible entry; `j`/`k` choose entries there and typed details remain accessible.
  Escape returns to the unchanged reading position. Keep activation separate from
  capability for later remapping; scrolling never silently retargets evidence.

Touch `crates/hq-tui/src/model.rs`, `render.rs`, shell input normalization, and the
application/local API/store history paging path. Preserve canonical source order
and conversation identity across pages and refreshes, rather than deriving order
from names, timestamps, or arrival position. Update footer/help and
`docs/rust/inbox-conversation-surface.md`.

Dependencies: conversation display preferences. Complete when model, rendered-buffer,
store pagination, and installed terminal tests cover oversized Markdown, both key
families, configurable paging, history loading without jumps, tail/End, resize,
new-content indication, and inspection entry/exit. Replace tests that deliberately
encode the old distinction between arrow scrolling and `j`/`k` selection.

### Persistent conversation composition

Keep transcript and composer visible as two surfaces, even on small terminals.
Tab/Shift-Tab cycle focus. Open conversations with composer focus; route typing and
caret movement only to the focused editor. Grow that editor with content up to the
persisted height cap (default one-third of available height), then scroll internally.
Collapse the unfocused composer to one line, retaining draft and cursor. Geometry
changes preserve the logical reading position and both surfaces remain usable.

The composer sends to the current conversation. Message-specific replies and forking
are outside scope: do not add reply targeting or a “Replying to” workflow. Resolve
direct-message send requirements using authoritative conversation context, never a
visible/selected message; preserve distinct direct/project conversation semantics.
Escape from compose focuses the transcript; Escape from ordinary reading returns to
the conversation list. Successful send leaves compose focused and ready for another
message. Preserve autosave, failures, pending sends, and exact receipt guarantees;
only committed evidence consumes sent text.

Touch TUI draft/focus/input/layout state, shell/configuration plumbing, and typed
application/local API draft targets or message-continuation validation as necessary.
Update conversation documentation and contextual hints. Dependencies: display
preferences and conversation reading. Complete when tests cover both focus
directions, editing isolation, small-screen sizing, successive sends in the same
conversation, navigation and draft restoration, failed and uncertain sends, and
reading-position preservation while composing. The later global-footer task keeps
its standalone-composer and validation scope.

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
