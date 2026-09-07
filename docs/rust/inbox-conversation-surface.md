# Inbox and conversation surface

Status: implemented.

## Product intent

Inbox, Sent, and Archived are conversation catalogs. Each root shows stable selectable rows; an
open conversation is a typed child route that owns the full content pane. People should understand
who a conversation is with, what happened, what can be done next, and where exact technical
evidence lives without learning HQ's internal vocabulary.

## Canonical routes

```text
HQ / Inbox
HQ / Inbox / <conversation>
HQ / Inbox / <conversation> / Technical details
HQ / Sent
HQ / Sent / <conversation>
HQ / Archived
HQ / Archived / <conversation>
```

Changing list selection does not change route depth. Enter opens the selected conversation. Escape
from evidence returns to the conversation; Escape from the conversation returns to its mailbox
root. Direct links install a destination-rooted path, so Back never depends on the incidental
source screen.

The route carries stable conversation identity. Participant and project names in rows and
breadcrumbs are current presentation, never routing or authorship authority. Refresh, reconnect,
reorder, target removal, and late pages re-resolve by typed identity and cannot open a different
conversation.

## List information hierarchy

Each row presents, in order:

1. participant-first title derived from exact mailbox and conversation context;
2. optional project context and latest bounded safe message preview;
3. unread, delivery, archived, or exceptional status only while useful.

Personal notes remain distinct from direct and project conversations. Missing names use an honest
fallback rather than exposing or parsing an internal ID. Rows contain no reply authority.

Empty roots explain what belongs there and offer only applicable actions. Passive startup and
refresh retain coherent content and never replace it with a first-page loading screen.

## Conversation information hierarchy

The conversation header identifies the participant and optional project in ordinary language.
Sender names are omitted when at most one non-user sender has authored messages in the
conversation. When multiple non-user senders exist, sender changes show the named or honest
fallback participant, including `Unknown sender` for unresolved names. The local user's name is
omitted. Conversation-wide attribution and consecutive-sender grouping use exact typed mailbox
evidence, independently of display names or loaded pages. Empty sender rows are omitted, while
delivery and exceptional status remain visible. Message bodies are normalized for safe terminal display; Markdown links
and image URLs remain inert text.

Activity is a typed, compact, non-speaker entry. Running, success, failure, command, file, tool,
search, plan, diff, and other supported kinds retain their closed type and status. Ordinary rows
show a bounded summary. Enter activates Inspect mode; Enter again on applicable activity opens full-pane technical details containing
the retained structured fields, output, failure evidence, and stable correlation data.

The viewport is anchored by stable entry identity plus wrapped visual-row offset. In ordinary
reading, arrows and `j`/`k` scroll one rendered line without selecting messages. PageUp/PageDown
scroll one viewport with the configured overlap (one line by default, clamped to allow progress).
Home reaches the oldest loaded content; End reaches the bottom and resumes following new content.
While reading earlier content, updates preserve that position and show “New content · End latest”.
`↑` and `↓` indicate clipped content. Resize, cached redraw, and composition preserve the logical
anchor; they never infer it from screen coordinates. Live activity updates retain a presentation
identity derived from their exact source, operation, item, kind, logical key, and runtime. The current
canonical fact remains separate; superseded facts resolve through source-owned activity support. The first history page contains the latest
canonical entries in chronological order. Moving the top visible entry changes the observed
canonical fact, requesting a bounded window around that fact with older and newer continuation
evidence. The entry and wrapped row being read stay in place when that window arrives. Reaching
the edge of a historical window does not enable tail mode; End requests the latest window. Failed
history loads retain the reading position and expose `l` to retry the same anchor explicitly;
repeated retry keys do not duplicate an outstanding request.

Enter explicitly starts Inspect mode at a visible entry. Arrows and `j`/`k` select entries there;
Enter opens typed details, and Escape closes details before returning from inspection to the saved
reading position and tail mode. Ordinary reading does not highlight a selected message.

## Conversation-scoped adjacent content

Conversation composition is one of the two permitted adjacent views. Conversation and new-message
drafts retain their exact typed targets and draft identities. Unicode editing, multiline paste,
autosave, stale-context recovery, and response-loss handling preserve content. Only a committed
canonical receipt consumes the draft. Inspection never supplies a send target: composing from
inspection returns to the retained conversation editor, and agent completion does not retarget it.

Composer shortcuts appear once in the global footer for both conversation messages and
standalone notes or messages. The editor uses its full inner area for text; inline validation
reserves a row only while an error exists. At small sizes, compact feedback keeps the error and
caret visible together. Recipient/project context, save status, and byte count remain in the
composer heading when space permits. Approval-needed status is independent of ordinary shortcuts.

The other permitted adjacent view is a command approval proven to belong to this conversation by
exact project, agent, provider, session, and operation evidence. An unresolved, ambiguous, or
mismatched request becomes recovery evidence with refresh guidance; it is never assigned by label,
timestamp, arrival order, or selection.

Pending approval does not interrupt typing. The editor shows **Approval needed** in the alert
color near its bottom. Tab or Shift-Tab preserves the draft and caret and replaces the lower
editor with the approval choices. From then on, Tab and Shift-Tab cycle between the transcript
and approval; Escape returns to reading. Enter in the editor still sends a message and cannot
approve a command. After the presented request resolves, the composer regains focus with its
exact text and caret, including when the user was reading the transcript. A further request
requires another explicit handoff. Reading position and tail mode survive these surface changes.

Approval state and in-flight answers retain the complete request correlation, including its
conversation target. A reused request ID with a different agent, project, provider, session, or
operation cannot inherit an earlier response or presentation. Background requests do not suspend
an unrelated editor or take focus when they resolve.

Global questions, forms, confirmations, progress, outcomes, help, and recovery are typed full-pane
routes rather than overlays.

## Materialized observation and stale suppression

The mailbox list and selected conversation window arrive as one revision-coherent materialized
view in Inbox, Sent, and Archived. Selection publishes the desired conversation and canonical
reading fact, waking the observer independently of the command worker. Latest-page previews use
a bounded cache keyed by stable row identity and authoritative revision; historical windows do
not replace those previews. Responses for a prior anchor cannot displace the current window.

Stale, duplicate, mismatched, or old-generation observations cannot replace the selected page.
When a row disappears, the model chooses the row at its prior logical index, or the new final row,
then waits for that exact page. Invalidations are body-free hints to reread authoritative state.

Locally sent messages appear immediately as typed pending rows when the window reaches the latest
history. While reading a partial historical window, sending marks new content without inserting
a pending row or receipt among older entries. Exact committed evidence replaces pending rows; a
definite rejection restores the exact draft; ambiguous response loss reconciles without a duplicate.
Project delivery status comes from typed dispatch evidence.

## Conversation composition

Opening a conversation prepares its composer with input focus. The transcript and composer remain
visible together. Tab and Shift-Tab switch surfaces; Escape from the composer focuses the transcript,
and Escape from ordinary reading returns to the list. Retained draft text does not capture reading
keys. The focused composer grows with rendered text up to its configured height cap, then scrolls
internally to keep the caret visible. An unfocused composer occupies one row. Small layouts omit
editor decoration before sacrificing the editable row. The transcript heading also yields space
to keep a message row visible when the editor uses a large height limit.

Conversation drafts carry a typed conversation identity through the client, API, and store. The
source resolves the initiating root from transaction-consistent canonical state. Direct asynchronous
messages and personal notes continue that root; project input retains the exact project exchange.
Provider sessions use unambiguous question context or a unique causal root frontier, never the
visible message, display text, timestamps, or page position. Ambiguous requests retain the draft
without choosing arbitrary message authority.

A conversation send retains its text until the exact command commits. The committed receipt opens
a fresh composer for the same conversation. Reentering a retained editor preserves its cursor.
Switching conversations saves the preceding draft before preparing the new target, and keystrokes
cannot edit a draft belonging to another conversation. Opening an empty conversation editor does
not reserve a stored draft: its stable local identity is persisted on the first nonempty autosave
or before sending. Typing and erasing before that save leaves no empty stored draft. Existing
saved drafts reopen with their exact identity, version, and text.

If loading a draft fails, the composer offers Enter to retry loading the exact target. Changing
conversations can load the new target independently.

While the previous conversation's draft saves, Tab and Escape still reach the transcript. A failed
save retains that draft and offers Enter to retry saving; it cannot send or edit the previous
conversation's text from the new conversation. Completing the save preserves the user's current
surface focus. A save failure before sending releases the editor for corrections and cancels that
send intent; sending again requires an explicit action. Choosing a different draft while a save or
send is pending retains that choice until the exact completion arrives. Leaving the requesting
screen cancels the queued choice without cancelling the existing save or send. New personal notes,
direct messages, and new project conversations open in the standalone editor rather than appearing
under another conversation’s transcript. Returning to a conversation prepares its own typed draft.

## Responsive rendering and actions

The mailbox root and conversation detail are separate full-pane routes at compact and wide widths.
There is no persistent list/detail split, collapsed list preview, pane focus, or width-dependent
navigation. Width affects wrapping, clipping, and visible row count only.

The breadcrumb communicates the active canonical path plus connection/refresh context. The footer
lists only actions valid on that route: selection/open at a root; compose, archive/restore,
details, and scrolling where applicable in a conversation; Back when a parent exists. Help exposes
stable IDs and causal evidence separately.

Semantic roles distinguish selection, authorship, activity status, delivery, warnings, errors,
draft focus, and technical evidence. No-color themes retain non-color cues. Rendering is borrowed,
terminal-safe, and model-pure.

## Acceptance

- Every visible Inbox-family depth is represented by the typed path or an explicit
  conversation-scoped adjacent surface.
- Enter pushes one meaningful level or submits the active composer; Escape pops one visible level
  or safely closes/saves input.
- Evidence is a full-pane child, activity never grants message authority, and names never identify
  a target.
- Refresh, reconnect, rapid selection, deletion, reorder, paging, and late effects cannot paint or
  navigate to the wrong identity.
- Compact and wide buffers have identical hierarchy and pending input; only geometry differs.
