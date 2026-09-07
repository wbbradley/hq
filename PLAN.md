# HQ

## Next Up

### Show conversation sender names only when needed

Conversation messages now use sender-rule headings, suppressing repeated headings
for consecutive messages from the same sender even across activity entries. They
still show names in a one-counterpart conversation. Hide ordinary sender names
unless more than one distinct non-user sender has authored messages in the
conversation; retain consecutive-sender grouping where attribution is needed.

- Determine distinct senders from typed mailbox addresses across the authoritative
  conversation, excluding the local human mailbox. Count actual message senders,
  not display names, activity entries, provider sessions, or only the loaded/visible
  page. Preserve conversation-wide evidence through pagination and refresh.
- Hide non-user names for zero or one other sender; show them for multiple other
  senders. Omit the local user's name, which the incoming sender-rule implementation
  now renders as `You`. Distinct senders
  with identical display names still trigger attribution. When labels are shown,
  use honest fallbacks for unresolved names; retain exact sender evidence in details.
- Omit the message header row when neither a sender label nor delivery/exceptional
  status remains. Preserve Pending, Received, Archived, and delivery-failure
  feedback without dangling separators, along with body styling and message spacing.
  Reuse the existing status-only/no-header layout instead of duplicating it; do not
  leave empty decorative sender rules when names are suppressed.
- Carry conversation-wide sender evidence through
  `crates/hq-application/src/snapshot.rs`, local API conversion/protocol, and
  `crates/hq-node/src/tui_client.rs` as needed. Update presentation and measured
  header geometry in `crates/hq-tui/src/model.rs` and `render.rs`.
  `UiConversationAuthor` currently retains only `You`, `Participant(String)`, or
  `Unknown`; do not infer distinct sender identities from these presentation values.
  Replace `same_message_sender`'s dependence on technical routing strings with the
  typed sender evidence used for conversation-wide attribution and grouping.
- Cover personal notes, one counterpart, multiple counterparts, duplicate display
  names, unresolved names, a second sender outside the loaded page, refresh adding
  a second sender, and status-only headers in mapper and rendered-buffer tests.
  Update affected conversation documentation and rendering expectations.

Dependencies: none. Coordinate measured header heights with the following
conversation-scrolling task so removal or appearance of sender rows participates
in stable viewport anchoring; preserve that task's scope.

Complete when one-counterpart conversations omit repetitive sender names and unused
header rows, while multiple-sender conversations retain attribution based on
conversation-wide typed evidence regardless of pagination, without losing delivery
feedback or message details.

### Conversation scrolling and persistent composition

Conversation navigation currently mixes wrapped-line scrolling (arrows) with
message selection (`j`/`k`), causing large jumps through long messages. Composition
also captures input while its draft exists rather than routing by surface focus.
Make ordinary conversation reading a scroll position, with a persistent composer
and separate, explicit message inspection.

Scope and agreed behavior:

- Keep two surfaces visible even on small terminals: transcript above, composer
  below. Tab/Shift-Tab cycle focus between them. Open conversations with composer
  focus. Route typing and caret movement only to the focused editor; transcript
  `j`/`k` and up/down all scroll one rendered line, including wrapping and explicit
  line breaks, without selecting messages.
- PageUp/PageDown scroll a viewport with configurable overlap, defaulting to one
  rendered line. Clamp overlap for tiny viewports so paging still advances. Load
  older history automatically near the top without moving the content being read;
  prevent duplicate requests and stop at exhausted history. Home reaches the oldest
  loaded content; End reaches the bottom and enables tail mode. These are transcript
  bindings; editor navigation retains its editing meaning.
- Follow incoming content only while already at the tail. Otherwise retain the
  prior reading position across appends, streaming updates, history prepends, and
  redraws, and show a new-content indicator with a discoverable jump-to-latest
  action. Typing and focus changes must not independently enable tail mode.
- Remove normal message-selection highlighting. Provide an explicit Inspect mode:
  provisionally Enter activates it and highlights a visible message, `j`/`k` choose
  entries within that mode, and details remain accessible through typed entry
  identities. Escape returns to the unchanged reading position. Keep activation
  separate from the capability so the key can be remapped later. Ordinary scrolling
  must not silently retarget inspected evidence.
- The composer sends to the current conversation. Message-specific replies and
  conversation forking are outside this task; do not add reply targeting or a
  “Replying to” workflow. Resolve existing direct-message send requirements using
  authoritative conversation context, never whichever message happens to be visible
  or selected. Preserve distinct direct/project conversation semantics.
- Grow the focused composer with content up to a configurable height cap, defaulting
  to one-third of available height, then scroll its editor internally. Collapse the
  unfocused composer to one line while retaining the draft and cursor. Keep both
  surfaces usable at small sizes and preserve the logical reading position when
  composer geometry changes. Expose and persist page overlap and composer height
  settings through the existing configuration path.
- Escape from the composer focuses the transcript; Escape from ordinary transcript
  reading returns to the conversation list. Successful sending leaves the composer
  focused and ready for another message. Retain existing autosave, send-failure,
  pending-send, and receipt guarantees; only committed evidence consumes sent text.
- Pending command approval uses the same lower surface, replacing the editor rather
  than opening a modal or introducing a third Tab stop. While composing, show
  “Approval needed” near the editor bottom in the alert semantic color without
  interrupting typing. Tab then preserves the draft/cursor and switches directly to
  the approval UI. In approval state, Tab/Shift-Tab cycle transcript and approval
  focus. After resolution restore the composer, focused, with its draft and cursor;
  preserve transcript position and tail state throughout. Keep exact typed approval
  correlation and ensure ordinary editing keystrokes cannot approve a command.

Implementation touchpoints: `crates/hq-tui/src/model.rs` (viewport, inspection,
focus/input routing, draft lifecycle, approval handling), `render.rs` (surface
geometry, highlighting, hints, alert), relevant shell input/configuration plumbing,
and `docs/rust/inbox-conversation-surface.md`. Reuse renderer-measured entry geometry
and the stable entry identity plus wrapped-row viewport anchor instead of creating
an unrelated positional selection model. Update stale footer/help guidance that
currently describes arrows and `j`/`k` as message navigation.

Dependencies: no preceding task is required. The following composer-shortcut task
touches the same rendering and footer code; preserve its scope. The approval-needed
alert is contextual status, not a duplicate row of ordinary composer shortcuts.

Complete when model and rendered-buffer tests demonstrate identical line scrolling
for both key families through oversized Markdown at narrow sizes; configurable
paging and history loading without jumps; tail/End and resize behavior; inspection
entry/exit without losing the reading position; focused input routing in both Tab
directions; persistent composition across sends, navigation, and failures; and
approval arrival while typing followed by explicit handoff and exact draft
restoration. Update the existing tests that intentionally distinguish arrow
scrolling from `j`/`k` selection, and cover configuration persistence and bounds.

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
