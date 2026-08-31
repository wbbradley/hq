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

### Define the Inbox conversation surface as a chat interface

Replace the protocol-shaped transcript presentation with a calm, readable messaging surface before
building the Projects workspace that will navigate into it.

- Make the Conversation pane the dominant wide-screen surface. Bound the Inbox list to a useful
  scanning width and give remaining columns to the transcript; retain an understandable compact
  layout and one-level focus navigation.
- Define participant-facing author labels such as `You` and `Alice` from typed identity context.
  Do not expose message purpose names, presentation kinds, shortened mailbox IDs, `message · open`,
  or `information only` as ordinary transcript chrome.
- Start ordinary message text at column zero of the pane's content area. Use whitespace,
  theme-derived author color, weight, and restrained background treatment to establish hierarchy
  instead of nested indentation or terminal boxes. Preserve a useful no-color presentation.
- Distinguish human/agent messages from tool and lifecycle activity. Decide what activity is shown
  inline, summarized, grouped, or progressively disclosed, while retaining exact command output,
  failure evidence, stable IDs, routing, and causal metadata in technical details.
- Define selection, scrolling, reply/archive affordances, paging, long-line wrapping, multiline
  content, loading/failure/empty states, and the modeless drafting relationship without making the
  transcript feel like a list of database records.
- Produce wide and compact wireframes plus semantic theme roles and an implementation migration in
  a focused design note linked from `docs/rust/tui.md`. Stop for review, then queue a separate
  test-first implementation task covering mapping, pure-model behavior, render snapshots,
  no-color/accessibility, and installed PTY behavior.

#### Implementation plan

1. Audit the transcript path from `ConversationContextDto` and `ConversationMessageDto` in
   `crates/hq-local-api/src/protocol/v1.rs`, through snapshot/page mapping in
   `crates/hq-node/src/tui_client.rs`, into `UiRow`, `UiConversationEntry`, focus/anchor behavior in
   `crates/hq-tui/src/model.rs`, and layout/rendering in `crates/hq-tui/src/render.rs`. Record which
   participant names and exact mailbox identities already exist at summary time, which entry
   taxonomy is currently promoted to visible text, and which visual problems require typed
   presentation data rather than parsing content.
2. Audit the current wide/compact dimensions, wrapping and entry-capacity assumptions, model and
   render tests in `crates/hq-tui/tests/{model,render_snapshots}.rs`, installed behavior in
   `crates/hq-node/tests/unix_tui_terminal.rs`, and the semantic theme catalog in
   `crates/hq-tui/src/theme.rs` plus `docs/tui-themes.md`. Treat no-color, narrow-screen, long
   multiline-message, selection, paging, and modeless-draft behavior as first-class constraints.
3. Create `docs/rust/inbox-conversation-surface.md` as a reviewable product specification. Define
   the participant-oriented conversation title and list-row hierarchy; the bounded Inbox-list and
   dominant transcript widths; column-zero message bodies; author, message, activity, selection,
   paging, and draft hierarchy; and exactly which protocol state moves to contextual technical
   details.
4. Specify an ordinary transcript presentation model that resolves `You`, named participants, and
   safe unnamed fallbacks from typed context. Separate message author/body/state from typed activity
   summary/detail/status, identify which redundant activity may be omitted only by typed kind, and
   prohibit inference from prose, sender ID prefixes, command strings, provider sessions, or message
   purpose labels.
5. Produce wide and compact terminal wireframes using the reported `hq · Alice` exchange, including
   user prose, agent progress, command activity, final response, a selected item, an expanded
   technical disclosure, and an open draft. Define full-row focus treatment that does not shift
   message text, content-aware wrapped-height scrolling, older-page loading, empty/loading/failure
   states, and a useful no-color equivalent.
6. Define semantic theme-role additions and deterministic terminal, no-color, and Base16 mappings;
   record text/icon fallbacks so color is supplementary. Add a staged implementation and removal
   plan covering local-API/presentation identity context, mapper/model changes, responsive render,
   obsolete `asynchronous · ID`, `message · open`, indentation, and `Conversation · complete`
   removal, plus focused model/render/installed-PTY acceptance gates.
7. Link the specification from `docs/rust/tui.md` and reconcile nearby prose that currently requires
   protocol-shaped `update · information only` rendering or describes fixed entry-height behavior.
   Run documentation formatting checks, architecture/qualification validation, strict locked
   workspace Clippy, and the complete locked workspace test suite; commit the review draft with a
   Conventional Commit, then stop for explicit review of the width, author/header hierarchy,
   activity treatment, selection styling, and compact behavior.
8. After explicit approval, incorporate revisions, add a separate front-of-queue implementation
   task, and archive this design task verbatim in `COMPLETED.md`. Do not modify production transcript
   behavior under design-task approval alone.

Risks and open questions: the summary DTO already has a resolved participant but the page DTO does
not carry that display context; successful turn-completion activity is currently distinguishable
only by prose and therefore cannot be safely hidden without adding a typed activity kind; the
renderer estimates every ordinary entry as three rows even when content wraps; and background-only
selection would be inaccessible in no-color themes unless paired with weight, underline, or another
non-color cue that does not re-indent the body.

### Implement the approved Projects workspace

Replace `UiProjectModal::Details` with the modeless Projects workspace specified in
`docs/rust/projects-workspace.md`.

- Add typed, decoupled project summary, folder, assigned-agent, conversation-count, recovery, and
  technical-evidence presentation state. Preserve selection and focused objects by stable identity
  across reload, resize, stale completion, and asynchronous operation results.
- Implement persistent selection-driven list/detail panes on wide terminals and ordinary list,
  detail, management, and form screens with one-level Back on compact terminals. Keep forms,
  progress, normal results, and recoverable failures in their owning pane.
- Add typed zero/one/many project-conversation navigation into Inbox, including a visible clearable
  project filter. Keep all message composition in Inbox and never infer a canonical conversation
  from a display label, recency, assignment, or provider session.
- Implement labeled state-dependent project administration for folders, agent assignment,
  lifecycle, recovery, and technical details. Keep ordinary delivery automatic; expose retry only
  from typed stalled-delivery evidence. Use modal confirmation only for the approved bounded
  destructive or force decisions.
- Write failing pure-model and presentation tests first, then responsive render/snapshot,
  executor/local-client, no-color/accessibility-text, and installed PTY coverage for every current
  project-details command path and the specification's scenario matrix.
- Remove `UiProjectModal::Details`, its selected-resource state, shortcut wall and key branches,
  routine outcome dialogs, obsolete completion continuations, and all production rendering of
  `Project details`, only after replacement coverage passes.
- Update `docs/rust/tui.md`, `docs/rust/acceptance-scenarios.md`, and nearby architecture text to
  match the approved nouns, primary action, responsive layout, and progressive disclosure. Keep
  protocol/schema v1 shapes in place where changes are required; this pre-release work needs no
  backwards-compatibility layer or version bump.

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
