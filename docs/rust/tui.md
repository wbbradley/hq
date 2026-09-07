# Typed TUI navigation and rendering contract

Status: implemented.

`hq-tui` is a pure presentation state machine. It owns deterministic UI state and borrowed
Ratatui rendering; it does not own a terminal, clock, task runtime, connection, store, signer,
filesystem, provider, or domain mutation capability. `hq-node` normalizes terminal and local-API
events, executes returned effects, and owns terminal restoration.

## State and effect boundary

The behavioral boundary is:

```text
UiModel + UiEvent -> Result<(UiModel, [UiEffect]), UiError>
```

Every asynchronous request has a nonzero process-local `EffectId`. A completion may change state
only when its effect kind and exact identity are still outstanding. Project and provider operations
also retain their stable domain operation, project, agent, provider, session, draft, or message
identities as applicable. Display labels, timestamps, arrival order, and list positions never
correlate a completion or grant authority.

Invalidations contain only typed scope and revision hints. They trigger rereading authoritative
state and never carry prompts, message bodies, secrets, or replacement navigation. Reconnects are
generation-scoped. Stale snapshots, pages, timers, and operation results cannot overwrite newer
state or move the active route.

Rendering borrows the model and performs no I/O or mutation. Resize changes geometry only: the
route path, selection identities, viewport anchor, drafts, form fields, pending operations, and
Back destinations remain unchanged.

## Conversation display preferences

Config exposes **Page overlap** and **Composer height limit** as numeric overrides.
Page overlap accepts 0–65535 rendered lines; an empty value restores one line.
Composer height accepts 1–99 percent; an empty value restores exactly one-third of
available height. Invalid input stays in the editor with inline guidance, and
Escape cancels the edit. Updates persist only the selected field and preserve
provider, model, and theme defaults. `hq config get` includes both overrides;
`null` in JSON means the default applies.

These preferences are stored through installation configuration and the local API.
Their transcript paging and editor-sizing consumers are tracked in the unfinished
conversation reading and persistent composition tasks.

## Canonical navigation path

One encapsulated typed route stack is the sole navigation history. It begins at exactly one root:

- `HQ / Inbox`, `Sent`, or `Archived`
- `HQ / Agents`
- `HQ / Projects`
- `HQ / Config`
- `HQ / New`
- `HQ / Help`

Roots retain stable list selection but do not acquire depth when the highlight changes or passive
data refreshes. Enter pushes a meaningful child such as a conversation, exact agent, exact project,
technical evidence, or the first step of a workflow. Escape pops one visible level. It never
reconstructs a screen from a focus enum, timestamp, cached label, or incidental origin history.

Global view shortcuts replace the whole path with the destination root. Cross-workspace links
install destination-rooted canonical paths, for example `HQ / Projects / <project>` or
`HQ / Inbox / <conversation>`. Back therefore remains within the destination hierarchy rather
than returning to whichever screen happened to contain the link.

The route carries stable typed targets. Names in breadcrumbs and rows are current authoritative
presentation only. A renamed or removed target is re-resolved by identity; if it no longer exists,
the model falls back to its valid parent or a typed recovery surface without guessing.

## Workflows and completion

Forms, choices, confirmations, progress, outcomes, recovery, help, and technical evidence are
ordinary full-pane routes. Sequential steps replace the current workflow level when the preceding
screen is no longer a meaningful Back destination. Successful completion consumes transient
progress/outcome levels and returns to the refreshed exact object or canonical root specified by
the workflow. Back cannot reveal an already completed form or restart an operation.

Operation routes retain their exact effect and domain identities. A late completion that no longer
matches the active operation may update correlated durable state, but it cannot dismiss, replace,
or reopen another route. Nonterminal worktree and reconciliation polling replays the exact retained
command rather than rebuilding authority from form text.

Draft editing is durable and Unicode-safe. Autosave, submit, stale-target recovery, and response
loss preserve the exact draft and message identities. A committed receipt consumes a draft;
definite rejection restores editable content; an ambiguous response never creates a second message.

## Rendering contract

The active route is the sole ordinary owner of the content rectangle. Inbox, Projects, Agents,
Config, New, Help, forms, choices, confirmations, progress, outcomes, recovery, and evidence all
render as full panes at every supported width. There are no persistent master/detail layouts,
inactive previews, pane-focus borders, or centered modal layers.

There are exactly two adjacent-content exceptions, both scoped to an open conversation:

- the message composer may share the conversation surface;
- an approval tied by exact project, agent, provider, session, operation, and conversation evidence
  may render beside that conversation.

Unresolved or ambiguously correlated approvals render recovery evidence and never attach by name,
recency, arrival order, or current selection.

The header projects the typed path into plain-language breadcrumbs and reports connection/refresh
context. Breadcrumbs clip by terminal display cells, preserving the current destination and useful
parent context; they are never parsed back into navigation. The footer describes only actions and
results for the active route. Contextual help exposes stable IDs and causal evidence separately
from ordinary language.

Compact and wide terminals share the same route hierarchy, stack depth, Back behavior, active
input, and completion rules. Width changes wrapping, clipping, and available rows only. Minimum-size
rendering remains safe and does not mutate the model. Semantic theme roles preserve selection,
status, warning, failure, and focus meaning under custom and no-color themes.

## Canonical journeys

### Root, detail, and evidence

1. A workspace shortcut installs its root path.
2. Moving the highlight preserves that root.
3. Enter opens the selected identity as one child level.
4. Technical details push a full-pane evidence child.
5. Escape returns to detail, then to the root, one visible level at a time.

### Sequential work

1. `New` installs `HQ / New`; choosing an intention advances within that workflow.
2. Delegated project or agent creation uses the canonical destination hierarchy with typed return
   context, not cloned screens.
3. Each completed choice/form is replaced or consumed.
4. Terminal completion opens the exact authoritative object or its conversation draft.

### Deep links and reconnect

1. A project-to-conversation link installs `HQ / Inbox / <conversation>`; a conversation-to-project
   link installs `HQ / Projects / <project>`; agent links behave likewise.
2. Escape returns to the installed destination root.
3. During refresh or reconnect the last coherent state and exact path remain visible.
4. Reordered or removed targets re-resolve by stable identity. A stale page or operation result
   cannot steal navigation.

## Inbox data and viewport

A materialized Inbox observation installs the list and selected first conversation page from one
revision. Selection requests use stable conversation identity and latest-value observation. A
bounded revision-tagged cache may retain coherent first pages; stale or mismatched pages never
paint under another row. When a selected row disappears, fallback is deterministic by its prior
logical position, followed by identity-scoped page resolution.

Conversation entries remain in reducer order. Authors derive from exact mailbox evidence. Activity
kinds and statuses remain typed and non-actionable; full retained command, file, tool, search, and
failure evidence is available through a child route. The transcript viewport uses stable entry
identity plus wrapped-row offset, can traverse oversized entries continuously, and preserves its
anchor across resize, paging, reconnect, and composer changes.

## Project, agent, and configuration boundaries

Projects own work, folders/resources, assignment, and lifecycle. Git worktree creation is an
optional project operation. Agents are named workers with independent administration and direct
conversation history. Direct messages and personal notes are first-class and are not modeled as
projects. Links compose these areas through typed routes instead of duplicating ownership.

Project lifecycle, resource conflicts, handoff, force decisions, provider/session management, and
configuration editing use typed full-pane workflows. Ordinary screens explain the intention and
next action; exact IDs, causal frontiers, provider/session identities, and recovery diagnostics are
progressively disclosed.

## Connection, execution, and shell obligations

The observation client and command client have independent ownership and bounded queues. A
generation-scoped wake makes selection and invalidation observable without reconnecting or waiting
for a blocked command. The executor coalesces redraws, releases timers once, replays only commands
whose protocol permits exact replay, drains on shutdown, and joins workers even after failure.

The shell exclusively owns terminal activation, input normalization, draw calls, and RAII
restoration. It restores the terminal after normal exit, Ctrl-C, setup failure, client failure, and
panic. Polling retries only interruption, preserves monotonic deadlines, and does not become a
periodic refresh loop. Terminal controls and links in user/provider content remain inert display
text.

Qualification lives in pure route/model tests, terminal-buffer snapshots at compact and wide
sizes, shell ownership/restoration tests, and installed pseudoterminal journeys. Tests assert typed
state and effect identities for authority; visible breadcrumbs and content qualify presentation.
