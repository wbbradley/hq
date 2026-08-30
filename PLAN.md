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

### Group project exchanges by their initiating conversation

- Add a cross-layer regression first for the exact reported corpus: two project inputs (`Let's have
  a conversation.` and `Let's have another conversation.`), the Codex response `Absolutely. What’s
  on your mind?`, and its completed activity currently render as two `Thread <hex>` rows plus one
  `codex · <provider-session>` row. Require two project conversations: the first input, its Codex
  response, and the response's activity together in one conversation; the independently initiated
  second input in another conversation. Every entry must appear exactly once in canonical order,
  with no residual project-associated raw-Thread or provider-session row. Prove the same grouping
  under shuffled arrival, rebuild, duplicate persistence, response loss, reconnect, handoff, and
  ambiguous concurrent bindings.
- Add a typed project-exchange identity, such as
  `ConversationKey::ProjectThread { project_id, thread_id }`, throughout the reducer/application
  query boundary, rebuildable store index and page query, local API DTOs/conversion, node client,
  and TUI model. A newly initiated project message must create or retain that stable exchange key;
  replies and correlated agent output/activity must join it. Starting another conversation for the
  same project must create another key. Direct-agent and non-project conversations retain their own
  typed identities; never merge merely by project ID, content, display name, current assignment,
  provider/session coincidence, or row position. Thread IDs remain technical evidence, not ordinary
  Inbox labels.

#### Implementation plan

1. Add the regression before changing grouping. In
   `crates/hq-testkit/tests/conversation_reduction.rs`, build the exact two-input corpus with Alice's
   correlated final answer and completed activity, reduce representative arrival permutations, and
   assert two `ConversationKey::ProjectThread` orders: the first input/output/activity together and
   the second input alone. Assert that neither a raw `Thread` nor `ProviderSession` order retains any
   project-associated entry. Extend the store query contract to persist, rebuild, reopen, and page a
   project-thread key without duplicating entries.
2. In `crates/hq-reducer/src/conversation.rs`, add
   `ConversationKey::ProjectThread { project_id, thread }`. Select it before ordinary address or
   provider-session grouping whenever a projected message has typed `project_id`; use the
   `MessageView`'s derived/declared thread. Route `HarnessActivityRecorded` with typed project
   attribution to the same `(project_id, thread_id)` key. Preserve the existing exact Thread and
   ProviderSession rules for non-project messages and activity. Keep ordering solely on the
   canonical presentation comparator so arrival order, reconnect, duplicate persistence, assignment
   handoff, and provider-session changes cannot split or reorder an exchange.
3. Carry the closed key through rebuildable persistence without a compatibility migration or schema
   version bump. Update `crates/hq-store/src/snapshot.rs` hashing and
   `crates/hq-store/src/database/repair.rs` key encoding, exact-row validation, loading, and digesting;
   change the current pre-release `reduction_conversation_keys` definition in
   `crates/hq-store/src/database.rs` in place to store a project ID and a third closed key kind.
   Existing local databases may be discarded. Extend corruption and pagination tests so a key digest
   cannot alias a direct thread/provider-session key and a cursor cannot cross project exchanges.
4. Extend local API v1 in place—no protocol-version bump—in
   `crates/hq-local-api/src/protocol/v1.rs` and `crates/hq-local-api/src/conversion.rs` with the exact
   project/thread IDs, exhaustive validation/conversion, canonical JSON round trips, and server
   routing coverage. Update `crates/hq-node/src/tui_client.rs` and its tests to retain a stable
   full-ID project-exchange identity for requests and selection. Use only a temporary plain
   `Project conversation` presentation label here; authoritative names and message previews belong
   to the following Inbox-summary task.
5. Update `docs/rust/conversation-model.md`, `docs/rust/storage.md`,
   `docs/protocol/local-api-v1.md`, and any now-stale provider-session wording in `docs/events.md` to
   describe project/thread identity as the grouping boundary and provider/session/operation as
   provenance within the exchange. Run focused reducer, store, local-API, and node tests first, then
   formatting, strict Clippy, and the complete locked workspace test suite.

Risks and invariants:

- A project ID alone is intentionally insufficient: independently initiated messages for one
  project remain separate exchanges, while replies reuse the selected exchange's thread.
- Only signed typed `project_id`/`thread_id` or `ProjectActivityAttribution` may select this key.
  Never infer it from content, an agent's current assignment, a provider session, or row adjacency.
- Project output/activity whose dispatch or binding is ambiguous remains invalid/conflicted in the
  existing reducers and therefore cannot manufacture or contaminate an Inbox conversation.
- This is a pre-release layout change. Do not add legacy-row repair, schema migration, dual decoding,
  or a new database/local-API version.

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
