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
show a bounded summary. Enter on applicable activity opens full-pane technical details containing
the retained structured fields, output, failure evidence, and stable correlation data.

The viewport is anchored by stable entry identity plus wrapped visual-row offset. Row scrolling and
entry selection can reach every part of an oversized entry and its neighbors. `↑` and `↓` indicate
clipped content. Paging, resize, cached redraw, and an open composer preserve the logical anchor;
they never infer it from screen coordinates.

## Conversation-scoped adjacent content

Conversation composition is one of the two permitted adjacent views. Reply and new-message drafts
retain their exact draft, conversation, message target, and project-thread identities. Unicode
editing, multiline paste, autosave, stale-target reselection, and response-loss handling preserve
content. Only a committed canonical receipt consumes the draft.

The other permitted adjacent view is a command approval proven to belong to this conversation by
exact project, agent, provider, session, and operation evidence. An unresolved, ambiguous, or
mismatched request becomes recovery evidence with refresh guidance; it is never assigned by label,
timestamp, arrival order, or selection.

Global questions, forms, confirmations, progress, outcomes, help, and recovery are typed full-pane
routes rather than overlays.

## Materialized observation and stale suppression

The list and selected first page arrive as one revision-coherent materialized view. Selection
publishes a latest-value desired conversation identity and wakes the observer independently of the
command worker. A bounded cache is keyed by stable row identity and authoritative revision.

Stale, duplicate, mismatched, or old-generation observations cannot replace the selected page.
When a row disappears, the model chooses the row at its prior logical index, or the new final row,
then waits for that exact page. Invalidations are body-free hints to reread authoritative state.

Locally sent messages appear immediately as typed pending rows. Exact committed evidence replaces
them; a definite rejection restores the exact draft; ambiguous response loss reconciles without a
duplicate. Project delivery status comes from typed dispatch evidence.

## Responsive rendering and actions

The mailbox root and conversation detail are separate full-pane routes at compact and wide widths.
There is no persistent list/detail split, collapsed list preview, pane focus, or width-dependent
navigation. Width affects wrapping, clipping, and visible row count only.

The breadcrumb communicates the active canonical path plus connection/refresh context. The footer
lists only actions valid on that route: selection/open at a root; reply, compose, archive/restore,
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
