# Pure TUI model and rendering contract

Status: Active pure-client contract

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

`UiEvent` covers one-time startup, normalized input, complete resizes, identity-bearing timer,
snapshot, conversation, draft, mailbox-command, named-agent administration, and managed-session
completions,
revision-only invalidations, and
generation-scoped connection states and failures. `UiEffect` covers complete all-section snapshot
requests, bounded reducer-ordered conversation-page requests, draft open/autosave, stable mailbox
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
and conversation scroll anchor by stable row/fact identity,
not by screen coordinate or vector index. Reload keeps each identity while it remains present and
falls back to the first logical item when it disappears. An invalidation cancels the model's claim
on an in-flight old conversation page; after the required snapshot arrives, the selected
conversation reloads from its first page and retains the fact anchor when still present. Reconnect
uses the same repair path. Resize changes dimensions only; it does not rewrite logical focus,
selection, the open conversation, typed-detail disclosure, applicable draft identity, modal state,
edited text, direct target identity, or pending submission.

## Mailbox composition and actions

The pure model owns reply, direct-message, self-note, archive, and restore interaction state. Enter
opens a selected conversation summary so the operator can select an exact message. `r` opens an
applicable reply draft only for a typed message target whose purpose permits replies; `d` selects
one unconflicted named-agent mailbox by stable installation/mailbox identity; `N` opens a self-note;
`a` opens archive confirmation for an open message; and `u` opens restore confirmation for an
archived message. Archive changes only that message's reversible state; it does not delete the
thread or any message history. Using either state shortcut on a conversation summary shows a
transient help hint that is dismissed by the next input, rather than recording a persistent
failure. Activity
entries carry no `UiMessageTarget`, so no key sequence can turn activity into a reply or reversible
state target. Escape cancels selectors and confirmations without a canonical mutation.

Draft editing accepts Unicode characters, bounded paste, and Unicode-safe Left, Right, Home, End,
Backspace, and Delete operations up to the ordinary content bound. A coalesced 250 ms timer
autosaves optimistic complete replacements. Submit waits for
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
Unicode-safe Backspace and Delete, and atomic bounded paste. Modal handling precedes global
navigation, so these keys cannot accidentally change sections while a dialog is open. Text, focus,
caret positions, field errors, and pending submissions survive resize and authoritative refresh;
async rejection keeps the user's input available for correction.

Focused fields have a visible selection treatment and insertion caret without adding a character
to the field's displayed value. Empty fields say whether input is required or optional; that hint
disappears once the field contains text. Concise guidance and examples appear with the focused
field, and known validation failures render next to that field before an effect is emitted. Stable failures from an
actual operation remain in the global recovery presentation because they are not form validation.

## Guided `New...` workflow

`n` opens one global intent launcher from every ordinary section. Its choices remain separate:
`Work with an agent on a project`, `Send a direct message`, and `Write a personal note`. The direct
path enters the typed recipient catalog, whose recipient union can grow to include reachable human
peers without inventing provider sessions for them. The note path enters the existing durable
self-note composer. Expert `d` and `N` shortcuts retain direct access to those independent
capabilities.

The project path is a pure coordinator over existing domain operations. It selects an existing
non-archived project or enters ordinary project creation, then selects an active agent whose
mailbox belongs to the project's home installation or enters ordinary agent creation. Unassigned
agents appear first. An agent already assigned elsewhere is never taken implicitly: the dialog
names the competing project and links to its explicit project handoff controls. Project creation,
agent creation, resource ownership, direct sessions, notes, and direct messages remain usable
without this coordinator.

A runnable current assignment skips setup and opens the ordinary new-message form for that project.
Otherwise the coordinator reuses a compatible historical project thread when one exists, or
selects an available provider using the shared typed provider policy and submits the existing
retry-safe activation or handoff command. A compact review names the project, agent, provider,
conversation behavior, and any assignment move without asking for message content. After the
authoritative snapshot proves the assignment runnable, the model opens the same empty project
message form; starting work never sends a project input by itself.

A transport failure, rejection, response loss, reconcilable result, or reconnect retains every
setup selection. Decision-bearing and recovery outcomes remain modal; Escape returns from their
evidence to the retained review instead of restarting the wizard.

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

`hq-node::LocalNodeEventClient` is the long-lived subscribed form of the ordinary local API client.
Its bounded poll distinguishes an idle socket from a disconnect, and its Unix connection retains an
incremental frame decoder so a read timeout cannot discard a partial frame. Reconnect delays remain
queued against monotonic deadlines across short shell polls. Every negotiated generation registers
the broad invalidation subscription before its acknowledged authoritative snapshot is exposed.

`LocalTuiClient` joins each complete authoritative local API snapshot with the passive node-local
provider catalog and maps them into one presentation bundle containing Inbox, Sent, Archived,
Agents, Projects, and typed provider choices. Section navigation selects an already
mapped slice and performs no client request, so it never replaces visible content with a loading
state. Invalidation and periodic repair load a replacement bundle in the background while the
previous complete bundle remains visible. Conversation summaries carry store-derived open,
archived, and local-human-authored counts, so mailbox filters do not scan or reorder message
bodies. Activating a summary issues the ordinary `ConversationPage` request with an opaque cursor. Returned
message/activity unions remain in reducer order; selected/coalesced activity is presented as
an `update` marked `information only` on ordinary screens and is never converted into a message
target. Its typed activity identity and status remain in technical details. The mapper also exposes only uniquely
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

`TuiEffectExecutor` owns one named worker and bounded command/result channels. The worker alone owns
the subscribed client; the shell cannot reach storage, domain planners, signers, relays, providers,
or files through this boundary. The executor preserves snapshot effect identity, releases each
timer once, coalesces redraw requests, and joins its worker on explicit shutdown or drop. Shutdown
drains bounded results while enqueueing the stop command, so saturated queues cannot deadlock the
join. Client failures and connection observations retain their generation so older results cannot
overwrite newer UI state.

The same worker executes draft list/save requests and stable mailbox commands through the ordinary
subscribed client. It allocates command/message identities, semantic time, and auxiliary randomness
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

The first responsive layouts are semantic Rust-era layouts, not Bubble Tea compatibility:

- at least 96 columns: persistent left section navigation and a content pane;
- 40 through 95 columns: compact horizontal section navigation above content;
- below 40 columns or 10 rows: a bounded resize message that retains the quit hint.

In the wide layout, Up/Down and `j`/`k` move through the vertical section list, while Left/Right
and `h`/`l` move focus between navigation and content. In compact layouts, Left/Right and `h`/`l`
continue to move through the horizontal section navigation.

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

Ordinary footers stay focused on immediate actions such as `Enter open`, `c create`, and `? help`;
the contextual overlay owns the complete shortcut reference. Guidance from an inapplicable
shortcut is transient presentation state rather than an operation failure. It explains the missing
prerequisite—for example, selecting a message rather than an activity update—and disappears on the
next meaningful input. Stable failures remain visually and behaviorally distinct.

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
Inbox and this first-run checklist:

```text
Get started with HQ
✓ Account ready
› Current: add a project and choose the folder or resource it owns
Press n New… and choose “Work with an agent on a project.”
```

The checklist advances from project/resource ownership to agent creation, agent-service readiness,
and the first project instruction. It shows only the current action plus completed prerequisites.
When more than one service is available, the guided workflow presents typed choices; one service is
automatic, and no service becomes a distinct setup step. Users never type a provider namespace.
The project step recommends recording an existing folder and keeps Git worktree creation behind the
advanced project option.

The following capture is the first dialog reached from that checklist. Its wording assumes the user
has never seen HQ, and its three intentions remain independent:

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

F1 replaces any dialog with plain-language help while retaining that dialog and every input. The
fresh-state installed acceptance scenario creates the account while the TUI remains open, reloads
the resulting ordinary workspace, restarts and reconnects the node, opens `New…`, and opens F1 help.
The wider acceptance ledger is deliberately split at stable boundaries: installed pseudoterminal
tests cover identity/account setup, folder-backed project creation, agent creation, first project
input, direct-message discovery, restart, reconnect, and terminal restoration; pure transition and
render tests cover multi-provider selection, exact-once setup and input dispatch, exact conversation
return, every onboarding stage, and help retention. These tests all begin from empty state or an
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
`Completed`, `InputSent`, `Ready`, and `Stopped` results close the submitting form immediately,
request an authoritative snapshot, and show a four-second bounded confirmation in the footer.
The refreshed snapshot selects the affected object: project creation selects the new project,
ordinary project changes reopen its details, project activation and handoff continue in the first
instruction composer, and manual session administration returns to the agent with the exact saved
conversation selected when known.

The navigation intent is retained independently of the footer timer and across connection loss. If
an already in-flight snapshot predates a completion and lacks the target, the model requests one
bounded follow-up snapshot; if the target is still absent it reports a typed stale-target recovery
instead of silently selecting something else. Stale effect identities remain inert.

Dialogs remain for results that still require attention or preserve evidence the user explicitly
requested: `Running`, `Rejected`, `Reconcilable`, and `Uncertain` outcomes, resource previews, and
fresh resource checks. Creation and close previews continue to require an explicit commit. Exact
operation, runtime, warning, and recovery identities stay in these exceptional dialogs and
technical help; routine success does not make the user dismiss those implementation details.

An activated conversation uses a
responsive second pane, centers rendering around the stable fact anchor, labels activity as an
information-only update, and expands only typed routing, semantics, evidence, or activity sections. Enter
opens a conversation or toggles its selected entry's details; PageDown requests the opaque next
page; Escape collapses details and then the conversation. The conversation footer spells out the
applicable `a archive` or `u restore` control for the selected exact message; contextual help carries
the wider reply, direct-message, note, navigation, and quit reference. Stable failures replace the
ordinary footer with a plain failure statement and recovery action; their stable code remains in
technical help. The Agents and Projects footers expose their
primary inspect, create, search, and help controls; responsive
modal tests cover wide and compact draft, agent-detail, and managed-switch rendering. Styling
supplements these text markers and is not the sole carrier of state.

### User-facing vocabulary and progressive disclosure

Ordinary TUI copy starts from the decision a first-time user is making. It uses `folders and
resources` for project-owned paths, `assigned agent` for who is responsible for project work,
`saved conversation` for durable provider/session history, `agent service` for a provider choice,
`working folder` for launch context, and `instructions` for work sent to a project. It says that HQ
is creating, saving, checking, sending, or confirming a change instead of exposing local-API,
reducer, reconciliation, authority, or provider-session mechanics. Boolean safety choices render
as `Yes` or `No`, and conflict copy names the competing project, account choice, or assigned agent
rather than reporting cardinality.

Dialogs lead with what will change and why HQ needs the input. Project details first show status,
folder ownership, and the assigned agent; agent details first show assignment-aware status and
saved conversations; mailbox dialogs say who will receive a message or that a personal note is
private. Destructive confirmations say what HQ keeps on disk and distinguish an explicit safety
override from ordinary confirmation. Compact dialogs omit or shorten secondary evidence before
they hide a required field, warning, action, or recovery instruction.

Technical evidence is preserved, not translated away. Detail views and the `?` then `t` page label
project, resource, assignment, message, request, provider/session, thread, revision, frontier,
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
action, and the command that returns to onboarding. Identity-only nodes remain supported: after the
first authoritative snapshot the TUI presents one primary `human create` action and an in-place F5
continuation instead of treating the absent human account as a shell failure. Join, selection, and
relay-recovery alternatives remain available when their typed state or contextual help makes them
relevant.

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
