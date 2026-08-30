# HQ

## Product direction

Design the TUI for people who have never seen HQ and do not know its internal vocabulary. Every
screen and dialog must make clear what the user is looking at, why HQ needs their input, what they
can do next, and what will happen afterward. Prefer user intentions and ordinary language over
authority, reducer, provider-session, assignment, thread, reconciliation, and other implementation
terms. Preserve exact technical evidence behind contextual details and recovery views.

Keep these user workflows distinct and composable:

- Projects define work and authoritative ownership of resources. Resource ownership is a core HQ
  concern; Git worktree creation and lifecycle management are not the product's center and should
  remain optional, progressively disclosed conveniences. Agents may eventually manage worktrees
  themselves.
- Agents are named workers that can be assigned to project work and contacted through
  conversations. Starting work should hide routine provider-session and assignment mechanics.
- Direct messaging, including future communication with other humans in the HQ network, remains a
  first-class path rather than an awkward special case of project work.
- Personal notes remain available without competing with the primary collaboration actions.

Never require a user to guess a valid identifier, namespace, state transition, or recovery command
when HQ already has enough typed information to present valid choices. Use progressive disclosure:
ordinary screens explain goals and next actions; details screens expose stable IDs, causal evidence,
provider/session identities, and recovery diagnostics.

## Next Up

### Make Inbox a human-readable, selection-driven master/detail workspace

- Separate stable conversation identity from row presentation. Extend the bounded conversation
  summary/read model with typed project and participant context plus a sanitized, clipped preview of
  the conversation's first meaningful line (falling back to the latest meaningful line when the
  opener is unavailable). Render human titles such as `Project name · Alice` for project work and
  `Me and Alice` for direct conversations, with the preview as secondary text. The two conversations
  from the reported corpus must therefore be distinguishable by `Let's have a conversation.` and
  `Let's have another conversation.` without exposing IDs. Do not use `Thread <hex>`, raw mailbox
  IDs, provider namespaces, or provider-session UUIDs as ordinary titles; retain exact values in
  technical details and use a plain unnamed-participant fallback when authoritative naming is
  unresolved.
- Make Inbox selection drive conversation loading. Reconciliation of the first nonempty Inbox
  snapshot must select a stable conversation and request its first page automatically; `j`/`k` or
  Up/Down in the Inbox list must immediately request the newly selected row. A newer selection may
  supersede an older pending preview, stale completions must be inert, rapid movement must not show
  the wrong page, and a completed preview must retain list focus rather than stealing conversation
  focus. Preserve the selected project conversation and logical message anchor across refresh,
  resize, reconnect, and authoritative reorder.
- Always render the Inbox conversation region, including bounded loading, empty, unavailable, and
  selected-diagnostic states; do not expand the list to full width while no page is loaded and do
  not require Enter to make the pane appear. Enter or `l`/Right moves focus into the already visible
  selected conversation. `h`/Left moves one hierarchical level at a time—drafting pane to
  conversation, conversation to Inbox list, and Inbox list to top-level navigation—rather than
  jumping from the conversation to `Inbox / Sent / Archived / Agents / Projects`. Apply the same
  back-to-list rule before section changes in compact layouts, and keep `j`/`k` scoped to the list
  or conversation that visibly owns focus.
- Render the wide Inbox as adjacent navigation, conversation-list, and conversation panes separated
  by single vertical dividers. Replace the conversation's `Block::bordered()` rectangle with an
  internal heading and a focused/unfocused left divider matching the navigation/list boundary; draw
  no top, bottom, or right box lines. Use one unboxed separator and an explicit back path in the
  compact stacked layout, preserving useful rows for messages instead of spending them on chrome.

Cover human labels and previews, eager selection loading, rapid stale-load replacement, stable
selection anchors, focus hierarchy, divider-only responsive layout, pure-model rendering, and
installed PTY navigation. Update `docs/rust/tui.md` so Inbox is documented as a persistent
master/detail workspace rather than a list that conditionally opens a modal-like rectangle.

#### Implementation plan

1. Extend the authoritative conversation-summary contract before changing presentation.
   - In `crates/hq-application/src/snapshot.rs`, add a closed presentation context that distinguishes
     personal notes, direct conversations, and project conversations. Retain exact project,
     participant-agent, and participant-mailbox identities as typed values while making project and
     participant names optional when authoritative resolution is ambiguous. Add a bounded optional
     one-line preview to `ConversationSummary` and `ClientProjection::Conversation`.
   - In `crates/hq-store/src/database.rs`, derive this context from the same serialized reducer
     snapshots used for the conversation index: resolve project names by exact project ID; resolve
     agent names only from singular agent/name/mailbox evidence; prefer a project exchange's
     historical output or dispatch participant and use the current singular assignment only when
     the exchange has no historical agent evidence. Recognize the local-human counterparty as a
     personal note. Never infer identity from provider prose, row order, or content.
   - Derive the preview deterministically from reducer presentation order. Prefer the first
     meaningful message line, fall back to the latest meaningful message when the opener is
     unavailable, collapse control/line-breaking whitespace, and clip on a UTF-8 boundary to the
     short-text byte bound. Keep counts and exact keys unchanged. Add store/application regression
     coverage for the reported two project exchanges, direct named/unnamed participants, personal
     notes, ambiguous names, multiline/control content, Unicode clipping, shuffled ingestion,
     repair, and reopen.
2. Carry presentation context through local API v1 in place and map it once at the node boundary.
   - Add closed `ConversationContextDto` and participant/project DTOs plus `preview` to
     `SnapshotItem::Conversation` in `crates/hq-local-api/src/protocol/v1.rs`; update exhaustive
     application conversion and strict validation in `conversion.rs` without a protocol-version
     bump. Bound every optional display string and reject incoherent project/direct/personal
     combinations. Extend canonical JSON, snapshot conversion, and server-session tests.
   - In `crates/hq-node/src/tui_client.rs`, continue deriving the stable row/request ID only from
     `ConversationKeyDto`, but derive ordinary titles only from the typed context: `Project · Alice`,
     `Me and Alice`, or `Personal notes`, with `unnamed participant` as the conflict-safe fallback.
     Put the sanitized preview in `UiRow.detail`, falling back to a plain count only when no message
     preview exists. Assert that raw thread, mailbox, provider, session, project, and agent IDs never
     enter ordinary row title/detail text.
3. Make Inbox selection own replaceable preview loading in `crates/hq-tui/src/model.rs`.
   - Add failing model tests showing that the first nonempty Inbox snapshot selects its stable first
     conversation and emits a first-page load without activation; moving with Up/Down or `j`/`k`
     immediately clears the old preview and emits a new exact-row load. Let a newer selection
     replace the pending request identity so late completions are inert; keep page-row mismatch
     validation for the current request.
   - Distinguish eager preview loads from explicit entry. An eager completion must retain the
     existing list/navigation focus, while Enter or `l`/Right on the selected loaded conversation
     moves to conversation focus. Activation during an eager load records the entry intent without
     showing another row. Preserve the selected row and logical message anchor across authoritative
     reorder, refresh, resize, reconnect, and same-row reload; only a genuinely removed or newly
     selected row clears the old page.
   - Give conversation loading/failure evidence row scope instead of relying only on the global last
     failure, so the renderer can distinguish loading, loaded-empty, selected diagnostic,
     unavailable/failed, and no-conversation states without showing stale content.
4. Make focus traversal hierarchical and consistent at every supported width.
   - Replace the current left/`h` jump with one-level transitions: conversation to Inbox list,
     Inbox list to top-level navigation, and only then compact top-level section movement. Make
     Right/`l` and Enter enter the already visible conversation from the list; keep top-level
     navigation, list movement, and conversation-entry movement scoped to the focus that visibly
     owns them. Preserve Tab/Shift-Tab cycling without allowing an absent conversation to become a
     focus target.
   - Extend pure-model tests across wide and compact widths for `h`/`l`, arrow keys, Enter, rapid
     selection, diagnostics, empty Inbox, section changes, stale effects, and refresh/reconnect
     anchor preservation. Update footer/help copy to describe `back to Inbox`, `open conversation`,
     and top-level section movement in the actual focus state.
5. Replace conditional boxed rendering with a persistent Inbox master/detail composition in
   `crates/hq-tui/src/render.rs`.
   - Wide layout must always reserve adjacent navigation, conversation-list, and conversation
     regions. Give the conversation region one left divider whose focused/unfocused role matches
     the navigation divider, render an internal `Conversation` heading and paging state, and draw no
     top, bottom, or right borders. Compact layout must always use a bounded stacked list/detail
     composition with one unboxed separator and visible back guidance.
   - Render loading, empty, unavailable, failed, and selected-diagnostic states inside the persistent
     detail region. Keep reducer entry order and technical disclosure unchanged once loaded, and
     calculate entry capacity from the divider/header layout rather than the removed bordered box.
     Add style-aware buffer assertions and update wide/compact snapshots so no conversation box
     corners or horizontal border rows remain and useful message rows increase.
   - Extend `crates/hq-node/tests/unix_tui_terminal.rs` with an installed Inbox scenario proving
     eager detail visibility, list-owned movement, and `h` returning from conversation to the Inbox
     before top-level navigation. Update `docs/rust/tui.md`, run focused application/store/local-API/
     node/TUI suites, formatting, strict locked workspace Clippy, and the complete locked
     all-target/all-feature workspace suite.

Risks and invariants:

- Presentation metadata is disposable authoritative read-model data, not conversation identity.
  Names and previews may change after repair or new evidence without changing row IDs, cursors, or
  page membership.
- Ambiguous or stale agent/project evidence degrades to plain unnamed labels. It must never select a
  participant, merge conversations, retarget a page request, or leak an internal identifier.
- A superseded load may still finish in the shell; only the model's latest exact effect/row pair may
  mutate the visible pane. No cancellation or response-order assumption is required.
- The Inbox pane remains present even when empty or failed. Focus may enter Conversation only when
  the selected row has a loaded conversation; diagnostics and loading placeholders are not fake
  conversation history.
- This is a pre-release local API v1/read-model change. Do not add a version bump, legacy decoder,
  migration, or backwards-compatibility branch.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

### Move all message composition into the Inbox workspace

- Extend the durable draft vocabulary with a typed project target keyed by stable project ID and
  carry it exhaustively through `hq-tui`, the node TUI effect executor, local API v1, application
  planning, and store encoding. Resolve the project's current mailbox, account audience, and human
  authoring authority from transaction-consistent state; do not pretend a project message is direct
  messaging to whichever agent happens to be assigned.
- Separate the shared composer from modal presentation: replace `UiMailboxModal::Compose` with a
  modeless drafting pane owned by the Inbox/conversation workspace while retaining its durable
  behavior for replies, direct messages, notes, and project drafts. Recipient selection or a
  destructive confirmation may remain a bounded dialog, but message writing itself must not be.
  Preserve autosave, restart recovery, optimistic conflicts, bounded Unicode editing,
  save-on-Escape, and atomic draft consumption. A reply or follow-up authored from an open project
  conversation must retain that exact project-exchange key and return to the same Inbox row. A
  separately invoked `New conversation` action may create another exchange for the same project;
  the distinction must be explicit rather than inferred from provider session or current selection.
- In the guided `n New…` flow, submitting the first project message must continue to establish the
  exact activation thread and dispatch the retained input once, but completion must select and load
  the typed project conversation in `UiSection::Inbox`. It must not open a fresh
  `UiProjectModal::SendInput`. An ordinary subsequent project send must remain in the same Inbox
  conversation instead of mapping `InputSent` to `UiProjectCompletionContinuation::Details`.
  Remove `open_project_message_composer`, `ComposeInput`, every `UiProjectModal::SendInput` entry
  point, its form/rendering code, and `n send instructions` after all entry paths use the Inbox pane.
- While only one agent service is available, skip the `Start project work` confirmation dialog and
  proceed with that sole valid choice. Keep this conditional on the typed service catalog so the
  choice can return when it becomes meaningful.
- Keep the project-send command underneath for CLI and application compatibility, exact first-thread
  activation, automatic ordered pending-input dispatch, idempotency, and response-loss recovery.
  Keep failures beside the conversation/composer with the draft intact and a plain next action; a
  stale, archived, conflicted, or unassigned project must not silently retarget another mailbox,
  discard text, fall back to Project details, or expose routine dispatch as a normal user step.
- Cover durable project draft encoding/restart, autosave/submit races, duplicate completion,
  existing runnable projects, `conflict/project_activation_thread_missing`, and the complete
  post-bootstrap single-service path in store, local API, node mapping, pure-model, render, and
  installed PTY tests. Update `docs/rust/tui.md` and the acceptance walkthrough so the modeless
  Inbox workspace is the sole project-message experience and common writing surface for ordinary
  mailbox actions.

### Define the Projects workspace interaction model

Rethink Projects as a mostly modeless workspace for finding a project, understanding its status and
ownership, and continuing its work. Produce and review the interaction specification before
queuing implementation; do not turn the current `Project details` modal and its shortcut wall into
a differently styled modal without first agreeing on the product nouns, verbs, and boundaries.

- Inventory every datum and command currently packed into `UiProjectModal::Details`. Classify each
  as primary project work, ordinary administration, exceptional recovery, destructive lifecycle,
  or technical evidence, and record why it belongs in Projects, Inbox, contextual help, or nowhere
  in the ordinary UI.
- Define a novice-facing noun model centered on **project** (the durable work/ownership container),
  **conversation** (where the user and agent collaborate), **agent** or **worker** (who handles the
  project), and **folder** (the ordinary-language view of an owned resource). Keep assignment,
  thread, provider session, dispatch, operation, and claim as technical terms unless an exceptional
  decision or recovery genuinely requires one.
- Define the primary verbs before laying out controls. Selecting a project should expose a clear
  summary, and its primary action should open or continue the project conversation in Inbox.
  Project administration should use explicit object-bearing labels such as `Add folder`, `Change
  folder path`, `Remove folder`, `Use as working folder`, `Change assigned agent`, `Close project`,
  `Reopen project`, and `Archive project`; verify the exact wording against the underlying semantic
  effect rather than preserving `add`, `replace`, `primary`, or `move` by inertia.
- Produce wide and compact interaction maps for a modeless master/detail workspace. On wide
  terminals, project selection should update an adjacent detail/administration pane. On compact
  terminals, use ordinary navigable screens with a clear back path rather than an overlay that
  captures the application. Reserve modal confirmations for bounded destructive or force decisions,
  and keep forms, progress, routine outcomes, and technical inspection in their owning pane where
  possible.
- Replace the undifferentiated shortcut block with labeled, state-dependent controls or a
  discoverable action menu. For each current key (`a`, `e`, `x`, `p`, `r`/`R`, `n`, `v`, `d`, `h`,
  `c`/`o`, and `z`), decide whether to retain it as a secondary accelerator, rename it, move it to a
  relevant subview, expose it only during recovery, or remove it. Pending dispatch should normally
  be automatic and appear as a plainly named retry only when recovery is needed; folder health
  checks should live with folder status; `n send instructions` must disappear entirely; and local
  shortcuts must not obscure or conflict with global meanings.
- Walk the proposed model through fresh creation, an active conversation, an unassigned or blocked
  project, multiple folders, ownership conflict, agent handoff, pending-delivery recovery,
  close/reopen, archive/unarchive, and narrow-terminal use. Record a noun/verb glossary,
  state-dependent action matrix, terminal wireframes, focus/key behavior, and progressive-disclosure
  rules in `docs/rust/tui.md` (or a linked focused design note), including a migration path away from
  `UiProjectModal::Details`.
- Stop for user review of that interaction specification. After its nouns, primary action, layout,
  and contextual action matrix are agreed, add a separate front-of-queue implementation task with
  test-first model, responsive render, installed PTY, accessibility, documentation, and removal
  coverage; do not silently treat approval of this research task as approval of an invented UI.
