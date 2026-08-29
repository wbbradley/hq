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
generation-scoped connection states and failures. `UiEffect` covers section-bound complete-snapshot
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

The pure model owns reply, direct-message, self-note, archive, and restore interaction state. `r`
opens an applicable reply draft only for a typed message target whose purpose permits replies; `d`
selects one unconflicted named-agent mailbox by stable installation/mailbox identity; `n` opens a
self-note; and `a`/`u` open explicit archive/restore confirmations for an exact message. Activity
entries carry no `UiMessageTarget`, so no key sequence can turn activity into a reply or reversible
state target. Escape cancels selectors and confirmations without a canonical mutation.

Draft editing accepts Unicode characters, paste, and character-aware backspace up to the ordinary
content bound. A coalesced 250 ms timer autosaves optimistic complete replacements. Submit waits for
the latest save acknowledgement, then emits one draft-backed command effect. Escape also waits for
the latest text to be durably saved before closing; an edit made while an earlier save is in flight
therefore cannot disappear. Save conflicts preserve the local editor text, adopt the current server
version, and require an explicit edit/retry. A rejected stale target leaves the draft and modal open
with a reselection action. Only a committed canonical receipt closes and consumes the draft.

## Named-agent catalog and administration

The Agents section carries complete passive agent, mailbox, lifecycle, runnable-selection, and
durable provider-session presentations alongside its rows. `/` performs case-insensitive search
over stable agent identities, permanent names, provider/session identities, and resolved display
names; the query and stable selected agent survive authoritative reorder, reconnect, and resize.
Enter opens identity-bound details, and session selection remains bound to exact provider/session
identity rather than vector position as the catalog reloads.

`c` composes one permanent agent name. Agent details use `r` to rename or explicitly clear the
selected durable session display name and `x` to open permanent-retirement confirmation. Retirement
does not emit an effect until Enter confirms it; `f` visibly opts into forced project/runtime
takeover, and Escape cancels without mutation. In-flight create, rename, and retirement modals are
not discarded during reconnect. Rejected, stale, conflicted, retired, or uncertain outcomes retain
the exact modal inputs and expose a stable failure code plus a corrective action.

Agent details also use `s` to start on an explicit provider, `e` to resume exactly the highlighted
provider/session, and `t` to stop that provider's local runtime without erasing durable history.
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

`LocalTuiClient` maps only complete authoritative local API snapshots into the exact requested
`UiSection`. Conversation summaries carry store-derived open, archived, and local-human-authored
counts, so Inbox, Archived, and Sent filters do not scan or reorder message bodies. Activating a
summary issues the ordinary `ConversationPage` request with an opaque cursor. Returned
message/activity unions remain in reducer order; selected/coalesced activity is presented as
non-actionable and is never converted into a message target. The mapper also exposes only uniquely
named, uniquely bound, non-retired agent mailboxes as passive direct-target candidates. The protocol
`AuthoritativeSnapshotDto` and presentation `UiSnapshot` are deliberately different records: one
is canonical local-API data, while the other is a small section-specific view containing only safe
rendering fields. Neither is a storage compatibility shape, and both passive records expose their
fields directly.

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

`render(Frame, &UiModel)` only borrows the complete model. It cannot update the model or invoke a
capability. Tests clone the model around every render and compare deterministic terminal buffers.

The first responsive layouts are semantic Rust-era layouts, not Bubble Tea compatibility:

- at least 96 columns: persistent left section navigation and a content pane;
- 40 through 95 columns: compact horizontal section navigation above content;
- below 40 columns or 10 rows: a bounded resize message that retains the quit hint.

The header reports the selected section, connection state, and authoritative revision. Rows expose
selection, stable presentation state, and bounded detail. An activated conversation uses a
responsive second pane, centers rendering around the stable fact anchor, labels activity as
non-actionable, and expands only typed routing, semantics, evidence, or activity sections. Enter
opens a conversation or toggles its selected entry's details; PageDown requests the opaque next
page; Escape collapses details and then the conversation. The footer exposes discoverable controls
for reply, direct, self-note, archive/restore, navigation, and quit, or the latest stable failure
code and operator action. The Agents footer exposes search, inspect, and create controls; responsive
modal tests cover wide and compact draft, agent-detail, and managed-switch rendering. Styling
supplements these text markers and is not the sole carrier of state.

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
