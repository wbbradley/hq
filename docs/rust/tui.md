# Pure TUI model and rendering contract

Status: Active pure-client contract

Focused interaction specifications extend this contract. The approved
[Inbox conversation surface](inbox-conversation-surface.md) is implemented. The approved
[Projects workspace](projects-workspace.md) remains queued for implementation. Until that
production acceptance gate passes, descriptions below that name current Projects rendering remain
implementation truth, not patterns to preserve.

`hq-tui` owns deterministic presentation state and borrowed Ratatui rendering. It does not own a
terminal, clock, task runtime, local connection, storage handle, signer, filesystem, process, or
domain mutation capability. The outer `hq-node` composition normalizes ordinary local API
observations into the closed event vocabulary and executes returned effects. Its Crossterm shell
adds terminal input and resize normalization plus exclusive Ratatui rendering ownership.

## Transition boundary

The only behavioral entry point is:

```text
UiModel + UiEvent -> Result<(UiModel, [UiEffect]), UiError>
```

`UiEvent` covers one-time startup, normalized input, complete resizes, coherent materialized Inbox
views, identity-bearing timer, snapshot, older-conversation-page, passive viewport geometry, draft, mailbox-command,
named-agent administration, and managed-session completions, revision-only invalidations, and
generation-scoped connection states and failures. `UiEffect` covers complete all-section snapshot
requests, latest-value selected-conversation observation, bounded older-page requests, draft open/autosave, stable mailbox
commands, typed named-agent and managed-session commands, timers, redraw requests, and exit. The transition function
performs no I/O and has no domain mutation port.

Every asynchronous request receives a nonzero process-local `EffectId`. A completion changes state
only while that exact identity is outstanding for its effect kind. Older snapshot or conversation
successes and failures cannot overwrite a newer request or connection state, and an elapsed timer cannot run
twice. Invalidation revisions coalesce into one greatest required revision. If an in-flight
snapshot is older than that requirement, its matching completion schedules one follow-up request;
it is never treated as current merely because the request succeeded. Connection observations obey
the shell's monotonic generation.

The model preserves each section's summary selection, focus, open conversation, typed-detail state,
and conversation viewport by stable entry identity plus visual-row offset, independently from the
selected fact and explicit follow-tail state, not by screen coordinate or vector index. Reload keeps
each identity while it remains present and
falls back to the first logical item when it disappears. The subscribed materialized view installs
the Inbox list and selected first page from one revision in one transition. The model retains at
most eight revision-tagged first pages by stable row identity, immediately clears an unrelated
transcript when an uncached row is selected, ignores stale or mismatched views, and selects the row at the old
index when the current row disappears. Invalidations and reconnect rely on that active observation
stream; ordinary snapshots may repair catalogs but cannot replace a newer coherent list/detail pair.
Resize changes dimensions only; it does not rewrite logical focus,
selection, the open conversation, typed-detail disclosure, applicable draft identity, modal state,
edited text, direct target identity, or pending submission.

## Mailbox composition and actions

The pure model owns reply, new-work composition, and whole-conversation archive interaction state. Inbox
selection moves the list highlight immediately and replaces a latest-value subscription interest;
the matching materialized view then replaces the transcript. A row whose conversation has advanced
by canonical presentation rank while another row is selected shows an explicit unread marker until
that row is viewed. Late pages may refresh the list but never replace the selected row or open its
follow-up draft. Rows for conversations that have not started are selectable without waiting for a
transcript page. Enter or
`l`/Right moves focus into that already visible conversation so the operator can select an exact message. `r` opens an
applicable reply draft only for a typed message target whose purpose permits replies. `n` opens the
shared New launcher for project work, direct messages, and personal notes. `d` confirms permanent
archive of the selected whole conversation; it never targets an individual message. The executor
first stops typed agent work and archives only after a definite stop. Activity entries carry no
reply authority. Escape cancels selectors and confirmations without a canonical mutation.

Draft editing accepts Unicode characters, bounded paste, and Unicode-safe Left, Right, Home, End,
Backspace, and Delete operations up to the ordinary content bound. A coalesced 250 ms timer
autosaves optimistic complete replacements. Ctrl-J and Shift-Enter insert a newline at the current
caret; plain Enter submits. Submit waits for
the latest save acknowledgement, then emits one draft-backed command effect. Escape also waits for
the latest text to be durably saved before closing; an edit made while an earlier save is in flight
therefore cannot disappear. Save conflicts preserve the local editor text, adopt the current server
version, and require an explicit edit/retry. A rejected stale target leaves the draft and modal open
with a reselection action. Only a committed canonical receipt closes and consumes the draft.

## Form interaction

All editable dialogs use one pure form editor rather than dialog-specific cursor policy. Tab and
Shift-Tab move forward and backward through fields; Up/Down and `k`/`j` change a focused choice or
list; Left and Right move the insertion caret while a text field is open. In a text field, `j` and
`k` remain literal input until a proper Vim editing mode exists. Text fields also support Home, End,
Unicode-safe Backspace and Delete, and atomic bounded paste. Modal handling precedes direct view
shortcuts, so `1`-`6` and every other key remain owned by the dialog while it is open. Active text
fields receive digits literally. Text, focus,
caret positions, field errors, and pending submissions survive resize and authoritative refresh;
async rejection keeps the user's input available for correction.

Each one-line field leaves a visible gap after its label and fills the dialog's remaining inner
width with a subdued surface. Focus changes that complete surface and composes a visible insertion
caret over it without adding a character to the field's value; no-color themes retain independent
non-color focus cues. An empty field right-aligns `required` or `optional` inside the surface, and
the hint disappears as soon as the field contains text. Padding, clipping, and caret visibility use
terminal display-cell width, so narrow dialogs and wide Unicode characters do not wrap or split a
value. Concise guidance and examples appear with the focused field, and known validation failures
render next to that field before an effect is emitted. Stable failures from an actual operation
remain in the global recovery presentation because they are not form validation.

## Guided `New...` workflow

`n` opens one global intent launcher from every ordinary section. Its choices remain separate:
`Work with an agent on a project`, `Send a direct message`, and `Write a personal note`. The direct
path enters the typed recipient catalog, whose recipient union can grow to include reachable human
peers without inventing provider sessions for them. The note path opens the durable, modeless draft
pane inside the Inbox workspace. Expert `d` and `N` shortcuts retain direct access to those
independent capabilities. Message text is never collected in a dialog.

The project path is a pure coordinator over existing domain operations. It selects an existing
non-archived project or enters ordinary project creation, then selects an active agent whose
mailbox belongs to the project's home installation or enters ordinary agent creation. Unassigned
agents appear first. An agent already assigned elsewhere is never taken implicitly: the dialog
names the competing project and links to its explicit project handoff controls. Project creation,
agent creation, resource ownership, direct sessions, notes, and direct messages remain usable
without this coordinator.

The coordinator owns navigation but not every surface. Project and agent pickers are true dialogs
and capture input; delegated Projects or Agents work is modeless and solely owns input while it is
visible. The coordinator retains stable selections and a typed return destination rather than a
stack of cloned screens. In guided project creation, Escape moves from a folder/worktree form to
the creation choice, from that choice to the project picker, and from the picker to the launcher.
After creation, Escape from the agent picker returns to the project picker with that exact project
selected. Completing a child consumes it, so Back cannot reveal its old form or outcome.

Worktree creation retains the exact command, operation, derived project, and action identities
returned by the daemon. A `Running` result becomes a non-cancellable progress surface. Bounded
polls replay the byte-identical retained local-API request, including after transport reconnect,
until the operation is completed, rejected, or requires reconciliation; they never rebuild a
command from form text. Completion then waits for an authoritative snapshot containing exactly the
derived project ID before opening “Choose or create an agent for <project>.” A same-named project,
arrival order, or current list position is never completion evidence.

A runnable current assignment skips setup and opens the ordinary Inbox draft pane for that project.
A compatible historical project thread resumes its exact provider, session, and thread without a
routine provider confirmation. A target without history creates one installation-local
conversation setup containing the exact project, agent, provider, and draft identities. Inbox
presents it as “Conversation not started,” separate from authoritative conversation rows. Esc
saves and closes the modeless composer without losing the setup; `r` or Enter reopens the exact
draft, `c` explicitly replaces the chosen agent while preserving that draft identity, and choosing
`n` plus the same project resumes it before agent selection. The model waits
for the authoritative snapshot to join the draft/message ID to its exact thread and activates or
hands off on that thread. The root row replaces the setup presentation, while the retained typed
setup remains restart-recovery evidence until a later snapshot proves the assignment runnable.

Within an open project conversation, `r` opens the modeless continuation draft. Sending it commits
one typed human project message; the project-owned post-commit operation sequences and dispatches
it automatically through the existing assignment and thread. The conversation shows working
activity and refreshes with status and final output in place. Those agent-authored outputs are never
eligible for project input. Ordinary messaging exposes no manual dispatch step; retry appears only
as a typed recovery action when canonical stalled-delivery evidence exists.

When a visible running agent turn becomes newly succeeded, failed, or interrupted and no other
agent turn remains running, the model opens the same exact continuation draft automatically. Direct
conversations target their latest replyable message. Initial views of completed history do not
trigger composition, and an existing draft is retained unchanged.

The same transition that emits a message command closes composition focus onto one local-human
`Pending` row anchored by the effect identity. The exact draft, target, action, and random draft/message
identity remain retained until a definite result. A durable ordinary-message receipt upgrades that
row to `Sent`; a project receipt keeps it `Pending` until the project catalog no longer reports the
input as queued. A definite rejection removes only the optimistic row and restores the exact draft;
response uncertainty retains one row and one command identity. Subscribed canonical pages reconcile
the row by message identity, never by its label or body, so receipt/invalidation races cannot flicker
or duplicate it.

One available agent service is selected automatically without a confirmation dialog. Several
available services open a service chooser; none leave that screen open with actionable setup
guidance. A real assignment move
still requires a compact review naming the project, agent, selected service, conversation behavior,
and handoff. Refresh failure or reconnect retains the exact project, agent, provider, accepted
message, and thread correlation and never authors the first instruction again.

A transport failure, rejection, response loss, reconcilable result, or reconnect retains every
setup selection. Recovery stays with the delegated child and retains the exact operation evidence;
it does not recreate an earlier dialog or restart the wizard.

Project path fields share one lexical input boundary. Exact `~` and `~/...` forms expand using the
current operating-system user's home directory, then `.` and `..` components normalize into the
absolute path shown in the form and submitted to the ordinary domain workflow. Other tilde forms,
relative paths, environment variables, command substitutions, and arbitrary shell syntax are not
expanded. This convenience does not replace filesystem canonicalization or resource-ownership
validation at the project boundary.

## Named-agent catalog and administration

The Agents section carries complete passive agent, mailbox, lifecycle, runnable-selection, and
durable provider-session presentations alongside its rows. `/` performs case-insensitive search
over stable agent identities, permanent names, provider/session identities, and resolved display
names; the query and stable selected agent survive authoritative reorder, reconnect, and resize.
Enter opens identity-bound details, and session selection remains bound to exact provider/session
identity rather than vector position as the catalog reloads.

Agent rows use assignment-aware language derived from the typed agent and project catalogs. An
active agent with no current project is `unassigned`; one current assignment names its project and
is `setting up` or `ready`; blocked, cardinality-conflicted, or identity-conflicted state is
`needs attention`; and an absorbing retirement is `retired`. The row never substitutes durable
session selection for runtime presence and therefore does not label an agent `running` or `idle`.
Agent details retain the exact project, assignment, provider, and session evidence behind that
summary.

`c` composes one permanent agent name. Agent details use `r` to rename or explicitly clear the
selected durable session display name and `x` to open permanent-retirement confirmation. Retirement
does not emit an effect until Enter confirms it; `f` visibly opts into forced project/runtime
takeover, and Escape cancels without mutation. In-flight create, rename, and retirement modals are
not discarded during reconnect. Rejected, stale, conflicted, retired, or uncertain outcomes retain
the exact modal inputs and expose a stable failure code plus a corrective action.

Agent details also use `s` to start through a typed agent-service choice, `e` to resume exactly the
highlighted provider/session, and `t` to stop that provider's local runtime without erasing durable
history. One available service is selected automatically; several open a chooser with the
configured default selected when available; none open an explanatory setup state. Unavailable
entries are visible context but cannot be selected, and ordinary input never accepts a raw provider
namespace.
Starting while a durable selection exists, or resuming a different durable session, requires an
explicit switch confirmation. This is deliberately conservative: the presentation never treats a
durable selection or `runnable` catalog flag as evidence that a process is currently live.

One stable managed-session effect remains pending across connection observations. The ordinary
client returns a retry-safe operation identity and one typed `Ready`, `Stopped`, `Rejected`, or
`Uncertain` outcome. Rejections retain category/code and a reload/reselect action; uncertainty
retains the reconciliation identity and directs the operator to keep the same request. A stale
process-local completion identity cannot replace a newer interaction. Visiting Agents and managing
a session does not discard the saved mailbox section's selection, focus, open conversation, or
logical anchor.

## Reconnecting client and effect executor

`hq-node::LocalNodeEventClient` is a read-only, long-lived subscribed local API owner. Its Unix
connection retains an incremental frame decoder, yields generation-scoped connection changes, and
registers the subscription before exposing the acknowledgement's authoritative base. The installed
composition retains that base, resolves and subscribes to the initial Inbox row, and obtains its
coherent page before terminal activation. A generation-scoped local socket-pair wake lets selection
changes interrupt an idle read normally without closing or reconnecting the daemon socket; the
separate close interrupt remains reserved for shutdown. The ordinary `LocalNodeClient` command
adapter and observation client negotiate, block, reconnect, and join independently.

`LocalTuiClient` joins each complete authoritative local API snapshot with the passive node-local
provider catalog and maps them into one presentation bundle containing Inbox, Sent, Archived,
Agents, Projects, and typed provider choices. A direct view shortcut selects an already
mapped slice and performs no client request, so it never replaces visible content with a loading
state. Invalidation and explicit manual repair load a replacement bundle in the background while
the previous complete bundle remains visible. Conversation summaries carry store-derived open,
archived, and local-human-authored counts, the exact reserved local-human mailbox, a typed
project/participant presentation context, and one sanitized bounded message preview. The node uses
the typed context for participant-first list titles such as `Alice`, `Project agent`, `Other
participant`, and `Personal notes`; optional project context and a bounded preview occupy the
second line. Unresolved names use an honest fallback rather than an internal identifier. Mailbox
filters do not scan or reorder message bodies.
Selecting a summary replaces a capacity-one desired-row slot and wakes the observation owner; rapid
selection therefore collapses to the latest stable row even while an ordinary command is blocked.
The observer shares an encapsulated typed key/presentation directory with the command adapter, but
has no mutation authority. It maps each pending command approval to exactly one conversation from
the request's agent, project, provider, and session identities, then verifies the operation against
the loaded running activity. The retained pending set is remapped whenever its snapshot or page
evidence changes. Missing, ambiguous, and operation-mismatched requests stay nonmodal as explicit
refreshable recovery evidence; display names, arrival order, and current selection are never used
to guess a target. Only PageDown sends an ordinary `ConversationPage` request, always with
a nonempty opaque older-history cursor. First-page loading text is never rendered; the retained
coherent detail remains visible until its replacement arrives.
Returned message/activity unions remain in reducer order. The page mapper classifies an author as
`You`, the named or fallback participant, or `Unknown sender` from exact mailbox evidence; labels
never become routing authority. It maps the closed status, agent-turn, progress, plan, diff, and
completed-item activity kinds without parsing content. The current live row shows the latest
non-empty progress text or `Agent is working…`; typed command rows show command source, up to three
terminal-safe output lines, exit/failure state, and a `… +N lines (t to view full output)`
disclosure. A selected command's scrollable inspector shows every retained command and output line
plus its exit status; provider-boundary truncation is identified as retained rather than complete.
File, tool, and web-search
rows show typed path counts/names/queries while preserving exact bounded detail and complete
correlation metadata in the inspector. Activity remains non-actionable. The mapper also exposes only uniquely
named, uniquely bound, non-retired agent mailboxes as passive direct-target candidates. The protocol
`AuthoritativeSnapshotDto` and presentation `UiSnapshot` are deliberately different records: one
is canonical local-API data, while the other is a small complete navigation cache containing only
safe rendering fields. Neither is a storage compatibility shape, and both passive records expose
their fields directly.

Provider choices follow one policy in both managed-agent and project-work forms: choose the
available configured default, otherwise the first available stable catalog entry; use it without a
dialog when it is the only choice; and block submission with setup guidance when no choice is
available. Refresh replaces a vanished or newly unavailable choice and reports that change, while
an exact saved-conversation resume retains its historical provider identity. Raw namespaces remain
visible only in technical details and advanced session administration.

For the Agents section, the mapper reuses the installed named-agent catalog projection rather than
reimplementing binding, selection, or display-name reduction. Exact provider/session identities
remain semantic command targets; only presentation names are terminal-sanitized. The worker maps
typed create, rename/clear, and retirement effects to the same existing CLI/client workflows and
ordinary local-API frames. Those workflows own stable request identity and response-loss
reconciliation; `hq-tui` never receives a planner, signer, project coordinator, or provider handle.
Managed-session effects follow the same rule: `local_client` translates passive start/exact-resume/
stop targets to the existing harness CLI workflow, which captures launch directory and environment
outside the TUI and submits the ordinary `AgentSession` frame. The TUI never stores or renders the
captured environment and does not own provider authority.

`TuiEffectExecutor` owns two named workers, a bounded command queue, and one bounded result queue.
The command worker exclusively owns `LocalTuiClient` and serially executes snapshots,
conversations, draft operations, mailbox commands, project work, and provider-session operations.
The observation worker exclusively owns `LocalTuiObserver` and blocks in its subscribed socket read;
it does not wait for command-queue idle time or use a 25 ms notification poll. Both can publish typed
events independently, while reducer effect identities and generation checks still reject stale
completions.

The executor preserves snapshot effect identity, releases each timer once, coalesces redraw
requests, and joins both workers on explicit shutdown or drop. Shutdown sets shared cancellation,
interrupts the exact active observation socket, drains bounded results while enqueueing the command
stop, and joins both owners even when either panics. Saturated command or result queues cannot
deadlock the join. Its only timers are exact reconnect retry, draft-autosave, and
completion-dismissal deadlines; there is no recurring snapshot or redraw timer.

When either worker enqueues a model event or exits unexpectedly, it writes the executor's
nonblocking Unix wake pair. The outer shell waits on terminal input, that descriptor, or the next
exact UI deadline. It drains and reduces every ready event, then redraws before sleeping again.
Consequently an interaction notification already received on the subscribed socket cannot wait for
terminal input, command completion, or a sampling interval before its first dialog frame.

Unix resize delivery may interrupt the descriptor wait with `EINTR`. The shell treats only that
result as transient, rechecks Crossterm's queued events and both readiness descriptors, and then
waits again. A finite wait keeps one monotonic deadline and recomputes its remaining duration after
every interruption; resize signals therefore cannot restart UI timers or turn the event-driven loop
into periodic polling. Other polling failures retain their typed terminal phase, error kind, and OS
code and still restore the terminal before exit.

The subscribed Unix client applies the same deadline rule independently around its daemon-socket
and control-wake poll. It retries only `EINTR`, re-evaluates both descriptors after every
interruption, and retains its incremental frame decoder and active connection generation. A real
connect, read, or write failure still enters the reconnecting state machine. Privacy-safe boundary
records distinguish connection observations from client workflow failures and retain the
generation, closed connection state, transport operation, and unavailable/transport/protocol cause;
they never include socket paths, frames, message bodies, prompts, or operating-system prose.

The command worker allocates command/message identities, semantic time, and auxiliary randomness
once, then the reconnecting runner retains and replays that exact command frame until a durable
receipt is known. No TUI component resolves human authority, thread roots, recipient validity, or
message-state frontiers. Those remain transaction-local node decisions.

## Data and encapsulation

Shell-normalized snapshots, rows, conversation pages/entries, message/direct targets, drafts,
mailbox commands, agents, agent mailboxes/sessions/actions, managed-session actions/results, typed technical sections, sizes, and
failures are passive records and expose public fields. Display text is bounded and sanitized before
entering this crate; user-authored draft content and validated provider/session identities retain
their exact bounded UTF-8. There is no accessor facade around those DTOs.

`UiModel` is not a passive record. It keeps the outstanding snapshot and timer identities aligned
with minimum revisions, reconnect generations, retry state, selection, and one-time startup/exit
state. Its fields therefore remain private and it exposes only observations needed by the shell,
renderer, and tests. `EffectId` likewise hides construction so zero or shell-invented identities
cannot enter a transition.

## Borrowed rendering and layouts

`render(Frame, &UiModel, &UiTheme)` only borrows the complete model and one immutable, fully
resolved semantic theme. It cannot update the model, resolve visual fallbacks, or invoke a
capability. `hq-tui` defines the closed role catalog and deterministic `terminal`, `no-color`, and
Base16-to-role mappings, but performs no configuration, environment, or filesystem I/O. The node
loads the selected native TOML, Base16 YAML, or bundled palette before it constructs the terminal;
reconnect, F5, snapshots, and drawing never reload it. Tests clone the model around every render
and compare deterministic terminal buffers and styles.

The renderer paints `ui.screen` across every cell before drawing content, and explicitly repaints
every cleared overlay with `ui.modal.surface`. All color, underline, and modifier decisions come
from semantic roles; a source guard prevents concrete `Color` values in `render.rs`. The complete
role and file-format reference is in [TUI themes](../tui-themes.md).

The responsive layouts give the complete content area to the current view:

- at least 96 columns: adjacent Inbox-list and conversation panes separated by one vertical
  divider; the Inbox list prefers 32 columns, stays within 24–36 columns, and preserves a 48-column
  Conversation target whenever space permits;
- 40 through 95 columns: a persistent stacked Inbox-list/conversation workspace with one separator;
- below 40 columns or 10 rows: a bounded resize message that retains the quit hint.

`1` Inbox, `2` Sent, `3` Archived, `4` Agents, `5` Projects, and `6` Config switch directly from
any modeless screen where text entry is not active. The current view is named in the header and
repeating its shortcut is inert. Modals capture every key, and text entry receives digits rather
than switching views. Config reloads the daemon-owned snapshot on entry and explicit refresh;
each save replaces only its selected field, so another client's unrelated acknowledged change is
preserved. Relay policy is administered through `hq relay add/remove`, not a duplicate Config field.
Within Inbox, Right/Enter moves from the list to the selected conversation;
Left/Escape returns to the list, which is the visible root. The conversation region is always
present and renders
its loading, empty, unavailable, or selected state without a surrounding box. Participant-authored
message bodies render Markdown in column-zero author/body blocks. Paragraphs, breaks, headings,
emphasis, code, quotes, ordered/unordered/task lists, links, images, and GFM tables use one
width-specific Ratatui text artifact for both display-cell measurement and continuous entry-slice
painting. The viewport's stable entry-plus-row start may cut through an oversized item, every
intersecting row is painted, and `↑`/`↓` cues mark clipped content without introducing blank rows.
Wide tables clip inside the pane; wrapped nested-list lines retain their structural indentation. A
focused item receives a full-row semantic selection surface without a marker or text shift. Compact
typed activity is not parsed as Markdown and uses status-specific semantic roles.
Older-page loading or failure retains the transcript and advertises only the actionable PageDown
state.

The node presentation boundary normalizes message line endings, expands tabs, and neutralizes
terminal controls before Markdown parsing. Rendering exposes link destinations and image URLs as
ordinary inert text; it never emits OSC-8 links, loads files or images, opens a network resource, or
loads a syntax theme. Raw HTML remains readable text. Drafts continue to store and edit their exact
raw Markdown source; the draft pane neutralizes controls and expands tabs only in its display copy.
Bracketed paste preserves multiline source. These are presentation rules only and do not change
canonical content, routing, reply targets, or activity semantics.

The header reports the selected section and plain device state (`Connected`, `Connecting…`,
`Reconnecting…`, `Offline`, `Update required`, or `Updating…`) without hiding retained rows.
The authoritative revision and raw connection state appear only in technical help. Rows expose
selection, plain presentation state, and bounded detail. `?` opens contextual help from every
ordinary section screen, whether or not that section contains or selects an item. F1 opens help
from ordinary screens and every dialog, so text fields can continue to accept a literal `?`. The
first help page explains the current screen or dialog, the selected item's plain-language state,
and the available actions. `t` switches to a separate technical page containing stable identity,
authoritative revision, connection state, and current recovery evidence; F1, `?`, or Escape closes
help. Help freezes background user actions while it is open but survives resize and authoritative
refresh. F5 requests a complete authoritative reload without closing the current dialog or losing
its inputs.

Ordinary footers stay focused on immediate actions such as `Enter open`, `c create`, and `? help`,
and advertise `1–6 views`; the contextual overlay names the complete view mapping. Guidance from an
inapplicable shortcut is transient presentation state rather than an operation failure. It explains
the missing prerequisite—for example, selecting a message rather than an activity update—and
disappears on the next meaningful input. Stable failures remain visually and behaviorally distinct.

Empty sections never collapse to a generic `No items` label. Inbox, Sent, and Archived each explain
the kind of conversation that normally appears there and name only an action the user can take from
that screen. Empty Agents offers creation of a named worker. Empty Projects explains that a project
records work and ownership of folders and resources, then offers creation from a folder without
making Git worktree management the primary concept. Empty-state footers and contextual help omit
`Enter` actions when there is no selected item.

### First-run walkthrough

Bare interactive `hq` now treats setup as one ordered journey rather than a collection of domain
commands. Before the terminal is activated, missing device identity renders one focused setup
screen with the reason, exact action, and continuation:

```text
HQ needs a device identity before it can protect your account and messages.
Run `hq identity init` to create it, or import an existing identity.
Then run `hq` again; the next screen will guide account setup.
```

Both `hq identity init` and successful human-account output leave `Next: run hq` on screen. With an
identity but no account, HQ opens the ordinary TUI and keeps this action visible:

```text
No human account is selected on this device.
Your account holds your conversations and collaboration identity.
Create one now: hq human create
Keep this screen open; when setup finishes, press F5 to continue.
```

An invitation remains an alternative account-recovery path in contextual help; it does not compete
with the one primary first-device action. F5 replaces the account setup view with the ordinary
empty Inbox. It explains that no conversations need attention and offers `n New…`, direct message,
and personal note actions without inferring a project, agent, or provider setup sequence.
When more than one service is available, the guided workflow presents typed choices; one service is
automatic, and no service becomes a distinct setup step. Users never type a provider namespace.
The project step recommends recording an existing folder and keeps Git worktree creation behind the
advanced project option.

The following capture is the first bounded launcher reached from the empty Inbox. Its wording
assumes the user has never seen HQ, and its three intentions remain independent:

```text
┌ New… ──────────────────────────────────────────────────────────────┐
│ What would you like to do?                                        │
│ HQ will guide you through only the choices that intent needs.     │
│                                                                   │
│ › Work with an agent on a project                                 │
│   Send a direct message                                           │
│   Write a personal note                                           │
│                                                                   │
│ ↑/↓ or j/k choose · Enter continue · Esc back                     │
└───────────────────────────────────────────────────────────────────┘
F1 help · Esc back/cancel · q quit
```

F1 replaces an active decision surface with plain-language help while retaining it and every input. The
fresh-state installed acceptance scenario creates the account while the TUI remains open, reloads
the resulting ordinary workspace, restarts and reconnects the node, opens `New…`, and opens F1 help.
The wider acceptance ledger is deliberately split at stable boundaries: installed pseudoterminal
tests cover identity/account setup, folder-backed project creation, agent creation, first project
input, direct-message discovery, restart, reconnect, and terminal restoration; pure transition and
render tests cover multi-provider selection, exact-once setup and input dispatch, exact conversation
return, the ordinary empty Inbox, and help retention. These tests all begin from empty state or an
explicit empty authoritative snapshot, so seeded demo data cannot conceal a first-run dead end.

`c create` in Projects opens one intent-first chooser. Its recommended path records an existing
folder as the project's first owned resource; the optional advanced path creates an isolated Git
branch and worktree. `w` remains an expert shortcut for that advanced path, but it is absent from
the ordinary footer and empty state. The folder-backed form asks for the path first, derives a
default project name from its final path component, previews the normalized path and overlapping
resource-ownership rule, and states that HQ does not take over filesystem or Git maintenance.
Conflict previews name the conflicting project when it remains in the authoritative catalog and
always show the conflicting display path. The resulting project/resource command and ownership
model is identical regardless of which creation convenience opened the form.

The direct-message chooser treats recipients as a future-extensible typed catalog. When it is
empty, it explains that no reachable recipient exists, points to agent creation as the currently
available path, and notes that people in the user's HQ network may also appear there. It renders
only `Esc close`; selection and composition controls remain hidden and inert until a target exists.

### Contextual command completion

One typed presentation policy covers every project and managed-session result. Routine
`Completed`, `Ready`, and `Stopped` results close the submitting form immediately,
request an authoritative snapshot, and show a four-second bounded confirmation in the footer.
The refreshed snapshot selects the affected object: project creation selects the new project,
ordinary project changes retain its workspace selection, project activation and handoff select the exact Inbox
conversation, and manual session administration returns to the agent with the exact saved
conversation selected when known.

The selected destination and its typed per-view workspace are retained independently of the footer
timer and across connection loss. If
an already in-flight snapshot predates a completion and lacks the target, the model requests one
bounded follow-up snapshot; if the target is still absent it reports a typed stale-target recovery
instead of silently selecting something else. Stale effect identities remain inert.

The owning Projects pane retains results that still require attention or preserve evidence the user
explicitly requested: `Running`, `Rejected`, `Reconcilable`, and `Uncertain` outcomes, folder
previews, and fresh folder checks. Creation previews require an explicit commit. Exact operation,
runtime, warning, and recovery identities stay in the pane and technical help. Only bounded
destructive or force decisions use a modal confirmation.

The Inbox always uses a selection-driven master/detail layout: its responsive second pane shows
the selected conversation and a modeless draft pane occupies the lower portion of that detail pane
while composing. Project rows expose `r continue` for the exact selected thread and `c new
conversation` for a separate root. A newly committed root is selected by its returned message ID,
never by display text. An activated conversation follows the tail unless the reader has navigated
away; Up/Down moves one visual row, `j`/`k` selects adjacent entries, and Home/End reveals the
selected entry's bounds. Rendering shows participant-oriented headings, measured themed Markdown
message bodies, and
compact typed non-Markdown activity. The terminal owns a bounded 128-entry artifact cache keyed by
stable entry identity, exact body, pane width, and the five semantic Markdown styles. Content,
resize, or theme changes therefore rebuild the artifact without adding presentation state to the
model; ordinary redraws reuse exact measured lines. Enter opens a conversation or toggles its
selected entry's details; PageDown requests the opaque next page. Wide details occupy a bounded
lower inspector; compact
details, or details beside an open draft, use a secondary pane. `h`/Left/Escape closes details
before leaving the selected anchor, and another `h`/Left returns to the Inbox list. The inspector
shows exact routing, semantics, evidence, activity state, and raw detail without inserting records
into the transcript. The conversation footer spells out the
applicable `d archive conversation` control in Inbox; contextual help carries the wider reply,
New, navigation, and quit reference. Stable failures replace the
ordinary footer with a plain failure statement and recovery action; their stable code remains in
technical help. The Agents and Projects footers expose their

Transient activity is already consolidated below this model. Refresh replaces the one live tail
entry in place; terminal evidence removes it and continuation pages cannot duplicate it. A newer
human message prevents an older progress snapshot from being moved beneath that message. A command
approval replaces only its exact conversation's lower composer region, preserves the transcript
viewport and any saved draft, and accepts explicit focus with Tab. Escape leaves it unanswered;
global view shortcuts remain available, and returning to that conversation restores its approval.
Other conversations remain replyable. Other provider questions and permissions retain the bounded
modal dialog because their interaction scope is global. Submission is correlated by both effect and
request identity until an answered or stale outcome; transport failure restores the exact choice.
Exact provider/session/operation evidence remains visible as technical detail. A reader following
the tail moves to a genuine replacement; a reader on another fact keeps that stable fact anchor.
primary inspect, create, search, and help controls; responsive tests cover wide and compact
modeless-draft, agent-detail, and managed-switch rendering. Styling
supplements these text markers and is not the sole carrier of state.

### User-facing vocabulary and progressive disclosure

Ordinary TUI copy starts from the decision a first-time user is making. It uses `folders` for
project-owned paths, `agent` for who is responsible for project work,
`saved conversation` for durable provider/session history, `agent service` for a provider choice,
`working folder` for launch context, and `message` for conversation content. It says that HQ
is creating, saving, checking, sending, or confirming a change instead of exposing local-API,
reducer, reconciliation, authority, or provider-session mechanics. Boolean safety choices render
as `Yes` or `No`, and conflict copy names the competing project, account choice, or assigned agent
rather than reporting cardinality.

Mailbox composition is deliberately different from those decision dialogs: it stays modeless in
the Inbox and never uses a message-entry dialog.

Decision panes lead with what will change and why HQ needs the input. Agent details first show
assignment-aware status and saved conversations; mailbox dialogs say who will receive a message or
that a personal note is private. Destructive confirmations say what HQ keeps on disk and
distinguish an explicit safety override from ordinary confirmation. Compact dialogs omit or shorten
secondary evidence before they hide a required field, warning, action, or recovery instruction.

The modeless Projects workspace follows the approved [Projects workspace](projects-workspace.md).
Wide terminals keep the project list and selected summary visible together. Compact terminals use
separate Projects, project summary, Manage project, Folders, and form screens with `h`/Left as
one-level Back. Enter on a project collaborates through Inbox: zero conversations opens a filtered
Inbox draft, one opens the exact conversation, and many shows the visible project-filtered list.
Manage project exposes only labeled actions valid for the typed lifecycle and selected object;
ordinary delivery is automatic and raw provider sessions never stand in for conversations.

Technical evidence is preserved, not translated away. In a focused conversation, plain `t`
toggles the selected item's scrollable detail and `j`/`k` scroll it; inside contextual help, `t`
instead selects the technical-reference page. Those detail surfaces project, resource, assignment,
message, request, provider/session, thread, revision, frontier,
runtime, and recovery values as technical. Exceptional outcomes lead with `still finishing`,
`could not make this change`, or `could not confirm whether the change finished`, then show the
exact state/category/code, request identity, retained external state, and retry guidance. Routine
completion uses a short `Done` footer instead. Render contracts cover these boundaries at wide and
compact terminal sizes, including the rule that a stable failure code is absent from the ordinary
footer and present in technical help.

## Shell obligations

The terminal/client shell must:

1. send `Started` once and preserve the returned model between events;
2. normalize keys, resize observations, connection state, snapshots, and invalidations before
   calling the transition function;
3. execute effects outside `hq-tui` and return the exact effect identity on completion;
4. use the ordinary local API for snapshots and mutations, with no direct storage or domain side
   channel;
5. allocate monotonically increasing connection generations and ignore no model-requested retry;
6. coalesce redraw work without dropping the latest model; and
7. restore terminal state through shell-owned RAII on every exit path.

Terminal ownership, key-event decoding, event-loop composition, and restoration remain outside the
pure crate in `hq-node`. `TerminalGuard` is armed before activation so partial activation, normal
quit, Ctrl-C, terminal errors, client-worker failure, and panic unwinding all attempt restoration
exactly once. Restoration reverses cursor hiding, mouse capture, alternate-screen ownership, and
raw mode and is idempotent. The shell reaches state only through `TuiEffectExecutor`; reconnect
execution and snapshot mapping are composed there.

The installed `hq tui` role accepts only human output with both stdin and stdout attached to a
terminal. A bare `hq` selects the same role when both streams are terminals and retains the
noninteractive `list` role otherwise. The binary returns from the guarded shell before translating
failure into a process exit, so it never exits while terminal modes are still owned.

Before activating the terminal or attempting daemon ownership, the installed shell performs a
read-only validation of the installation identity. A missing identity fails immediately with the
stable `setup.identity_required` diagnostic, a plain-language reason, the `hq identity init`
action, and the command that returns to the Inbox setup journey. Identity-only nodes remain
supported: after the first authoritative snapshot the TUI presents one primary `human create`
action and an in-place F5 continuation instead of treating the absent human account as a shell
failure. Join, selection, and relay-recovery alternatives remain available when their typed state or
contextual help makes them relevant.

Human-account recovery is a closed typed presentation, not one aggregate ambiguous flag. The node
mapper distinguishes no local selection, unresolved selection candidates, multiple local selection
records, a selected account without local authority, pending membership, revoked membership, and
non-unique membership authority. Ordinary screens explain the exact condition and only applicable
create, join, select, sync, or repair action. They never ask the user to run `hq human show` merely
to discover which category failed.

Contextual technical help assigns each condition a stable `human_*` recovery code and retains the
directly relevant account candidates, selection frontier, membership status/frontier, and active
acceptance identities. Wide layouts show complete candidate account IDs for direct use with
`hq human select`; bounded layouts use short evidence and direct `hq human show` only when the full
bounded evidence set cannot fit. A local creator root or exactly one active local membership
acceptance produces `Ready`; evidence owned by another installation never does.
