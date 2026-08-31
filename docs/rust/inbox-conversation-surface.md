# Inbox conversation surface specification

Status: approved, implementation in progress. The typed conversation voices and activity boundary
are implemented. Responsive pane sizing, measured scrolling, dedicated theme roles, and the
technical inspector remain acceptance work.

## Product intent

Inbox is HQ's primary collaboration surface. It should read like a conversation between people,
not like a projection of message and activity records. The ordinary view answers four questions:

1. Who is this conversation with?
2. What did each person say?
3. Is work happening or does something need attention?
4. Where can I write next?

Purpose enums, presentation enums, reversible-state vocabulary, mailbox IDs, fact IDs, causal
frontiers, provider sessions, and operation IDs do not answer those questions. HQ must retain that
evidence, but it belongs in contextual technical details.

The visual model is a quiet full-width transcript rather than speech bubbles. Bubbles consume
scarce terminal width, make long technical prose ragged, and imply that color or horizontal
position carries authorship. HQ instead uses explicit author labels, whitespace, and theme roles.

## Pre-implementation audit

The presentation before this specification was approved created the reported result directly:

- `render_rows` gives the Inbox list 60 percent of the post-navigation width and the Conversation
  pane 40 percent.
- `render_conversation` prefixes its title and every state line with a space.
- `render_conversation_entry` prefixes entry headers with three cells and bodies with five, then
  renders `kind · state · content` as one record-like line.
- `tui_message_entry` constructs the visible header from a protocol purpose and a shortened sender
  mailbox: for example, `asynchronous · 000000000000` or `project output · 2a79b1a4651d`.
- Activity uses `activity · succeeded` followed by `update · information only · ...`; both phrases
  describe implementation semantics rather than the user's work.
- `Conversation · complete` exposes the absence of another page as if it were conversation status.
- Viewport capacity assumes every ordinary entry occupies three rows even though Ratatui may wrap
  its body over many more. Long messages can therefore be clipped or make anchor placement
  unpredictable.

The data was close to what the new surface needed, but it was split at the wrong boundary.
`ConversationContextDto` already carries a resolved project and participant name plus exact
participant mailbox evidence for each summary. `ConversationMessageDto` carries exact sender and
recipient mailboxes for each entry. The summary mapper discards its participant context after
building a row title, while the page mapper receives no display context and therefore falls back to
protocol purpose plus sender ID. The implementation must retain typed conversation display context
through the page presentation; it must not parse the current row title to recover it.

Activity had typed status but no typed activity kind or human summary/detail split. In particular,
`Codex turn completed` cannot safely be recognized as redundant without parsing prose. Any rule
that hides or rewrites activity therefore requires typed source data first.

## Information hierarchy

### Inbox list row

The list is for scanning conversations, so each row has two semantic lines:

1. participant name, or `Personal notes`;
2. optional project context followed by the latest bounded message preview.

For the reported exchange the row is:

```text
Alice
hq · Are there any local changes that are uncommitted?
```

The selected-row treatment spans the available row width. A leading `›` is not required for the
color theme or no-color theme because reverse, weight, and explicit focus guidance remain
available. If terminal capability testing later proves a marker necessary, it belongs in a stable
one-cell list gutter and must not be inserted into or removed from the row text as focus changes.

Counts such as `1 open messages` are fallback empty-preview or exceptional-state information, not
the usual subtitle. `open`, `sent`, and `archived` remain Inbox-section filters and message actions;
ordinary open messages do not need an `open` badge in every row.

### Conversation header

The heading names the participant. A second muted line supplies context only when useful:

```text
Alice
Project · hq
```

Direct conversation: `Alice`, with no redundant second line. Personal conversation: `Personal
notes`. Unresolved project participant: `Project agent` with `Project · hq`. Unresolved direct
participant: `Other participant`. Conflicted identity: `Participant needs attention`, accompanied
by a plain recovery line and exact evidence in technical details. Ordinary headings never contain
a mailbox prefix or stable ID.

`Conversation · complete` disappears. No paging label is shown when the loaded history is complete.
When another page exists, a muted, actionable line says `Older messages available · PageDown` at
the history boundary. Loading that page changes the line to `Loading older messages…`; failure
keeps the transcript and says `Older messages could not be loaded · PageDown retry`.

### Message

Every ordinary message uses this structure:

```text
You
Are there any local changes that are uncommitted?

Alice
I’ll check the repository’s working tree and summarize any staged, unstaged, or untracked changes.
```

Both author and body begin in column zero of the Conversation pane's content area: immediately
after the pane divider, with no renderer-added padding. Continuation lines created by wrapping also
begin there. A single blank row separates transcript items. Paragraph breaks inside a message are
preserved.

The author is explicit text and has a semantic theme role. `You` and the other participant use
distinct theme-derived author styles; the message body uses ordinary text. Color is supplementary,
not the source of identity. There are no `asynchronous`, `question`, `project output`, `message`,
`final answer`, sender-ID, or `open` labels in the ordinary message.

An archived message may show `Archived` after the author while it is selected or while viewing the
Archived section because that state changes the available action. A rejected message always shows
`Could not be delivered` and its plain recovery action. Absence of a badge means the normal state.

HQ should not invent timestamps: none exist in the current conversation projection. A later time
design must begin with authoritative typed time semantics rather than parsing fact order or local
receipt time.

### Activity

Activity is not a speaker and must not look like a chat message. Its ordinary form is one compact
line at column zero, separated by the same whitespace rhythm:

```text
● Checking repository status…
✓ Checked repository status
! Repository check failed · Review details
```

The symbol, verb, and style all reflect typed activity status. Running activity remains visible.
Failure and interruption always remain visible until superseded by authoritative state. Successful
tool activity remains as one subdued line so the transcript explains what happened without dumping
the command envelope into the conversation.

Raw executable paths, shell wrappers, exit-code framing, stdout/stderr, provider lifecycle text,
and stable failure reasons belong in an expandable activity inspector. That inspector may show the
exact original content and typed status; it may not discard or paraphrase technical evidence.

The activity projection must grow a closed typed kind and separate bounded summary/detail fields,
for example:

```text
Tool { summary, detail }
AgentTurn { phase }
System { summary, detail }
```

Exact names are an implementation decision, but the contract is not: rendering, grouping, and
omission use typed variants only. They never search activity prose or command strings.

A successful `AgentTurn::Completed` immediately following a final answer is redundant and may be
omitted from the ordinary transcript. It remains present in technical history. Without that typed
kind, the existing `Codex turn completed` line must remain visible in a compact activity form.
Failed or interrupted turns are never omitted.

### Technical details

Technical disclosure must not expand a message by inserting five-space-indented records into the
transcript. `t` opens an in-pane inspector for the selected item. On a wide terminal it occupies a
bounded lower region of the Conversation pane; on a compact terminal it is an ordinary secondary
screen with `h`/Left/Esc returning to the same transcript item. The inspector contains the existing
typed routing, semantics, evidence, and activity sections, including exact IDs and raw activity
detail.

The inspector is visually and behaviorally distinct from the modeless draft. Opening technical
details never closes or overwrites a saved draft. If the draft needs the same lower region, the
technical inspector uses the compact secondary-screen behavior even on a wide terminal.

## Responsive layout

### Wide terminals

At 96 columns and above, top-level navigation keeps its current bounded width. The remaining space
uses a bounded Inbox list rather than a percentage split:

- preferred list width: 32 columns;
- minimum list width: 24 columns;
- maximum list width: 36 columns;
- preserve at least 48 columns for Conversation whenever the terminal can do so;
- every additional column goes to Conversation.

This makes the transcript dominant at ordinary 100–160 column terminal widths while preventing the
list from becoming too narrow to distinguish names and previews. The exact constants must be
covered by boundary tests rather than scattered through rendering branches.

The reported conversation then reads approximately as follows. The vertical rules are the only
pane boundaries; there is no outer Conversation rectangle.

```text
 HQ             │ Inbox · 1               │ Alice
 Inbox          │                         │ Project · hq
 Sent           │ Alice                   │
 Archived       │ hq · Are there any      │ You
 Agents         │ local changes that are  │ Are there any local changes that are uncommitted?
 Projects       │ uncommitted?            │
                │                         │ Alice
                │                         │ I’ll check the repository’s working tree and summarize
                │                         │ any staged, unstaged, or untracked changes.
                │                         │
                │                         │ ✓ Checked repository status
                │                         │
                │                         │ Alice
                │                         │ No. The working tree is clean—there are no staged,
                │                         │ unstaged, or untracked changes. Current branch:
                │                         │ `wbbradley/projects-workspace-spec`.
```

The actual selected list row has a full-width selection style; the wireframe omits fake text
markers. The focused transcript item similarly receives a full-row selection surface without
changing its text origin or adding a marker.

When the final Alice message is selected and technical inspection is requested, its author and body
retain that full-row focus surface while a bounded lower inspector appears:

```text
Alice
No. The working tree is clean—there are no staged, unstaged, or untracked changes.

────────────────────────────────────────────────────────
Message details
Routing
sender 2a79… · recipient project account
Semantics
project 2a04… · final answer
Evidence
message 8b1c… · conversation e751…

h/← close details · ? help
```

The abbreviated values above illustrate ordinary inspector scanning. Selecting a technical section
reveals its exact unabridged value; copying or machine-oriented inspection must never receive the
abbreviation. The inspector divider is not a box around either message or transcript.

### Compact terminals

From 40 through 95 columns, Inbox retains its always-visible stacked list/transcript relationship.
When the list owns focus, it receives the flexible upper region and Conversation retains a
four-row preview. When Conversation owns focus, the list contracts to a summary header plus the
selected row and Conversation receives the remaining height. A single horizontal divider
separates them.

```text
Inbox · Sent · Archived · Agents · Projects
────────────────────────────────────────────
Inbox · 1
Alice · hq
Are there any local changes that are…
────────────────────────────────────────────
Alice · Project hq

You
Are there any local changes that are
uncommitted?

Alice
I’ll check the repository’s working tree…
```

The transcript remains visible while the list owns focus, as already required. `l`/Right/Enter
moves from list to transcript; `h`/Left returns to the list; another `h`/Left returns to top-level
navigation. Nothing dismisses or unloads the selected conversation merely to change focus.

Below 40 columns or 10 rows, the existing bounded resize message remains appropriate.

### Modeless draft

The draft remains the lower part of the Conversation pane and uses the same content origin:

```text
────────────────────────────────────────────────────────
Message Alice · saved
Let's check the ignored files too.

Enter send · Esc save and close
```

Opening a draft reduces transcript height but does not hide its selected conversation. The divider
is the only box edge. Draft target, persistence state, and validation stay in its header/footer;
the transcript does not repeat them.

## Focus, selection, and scrolling

- List selection and transcript focus are different. Moving through Inbox rows immediately updates
  Conversation, but the transcript shows no focused-message surface until Conversation owns focus.
- Enter or `l`/Right enters the transcript at its stable fact anchor. `j`/`k` and Up/Down move among
  selectable messages and activity. `h`/Left returns one visible level.
- Focus uses a full-width semantic selection surface. It does not add `›`, indentation, a border,
  or any other glyph that reflows body text. The no-color theme uses reverse plus weight; explicit
  author and footer text preserve meaning without style inspection.
- Reply/archive/restore controls appear in the footer only when valid for the exact selected
  message. The normal `open` state is not repeated in the transcript. Activity exposes technical
  inspection but never reply or reversible-state actions.
- Rendering measures each item's actual wrapped display-cell height at the current pane width,
  including preserved paragraph breaks, author line, status line when present, and selection
  inspector. It centers or reveals the stable fact anchor by visual rows rather than assuming three
  rows per entry.
- Resize recalculates visual row spans without changing the fact anchor. Page loading prepends or
  appends according to authoritative reducer order and preserves the selected fact's screen
  position where practical.
- Very long unbroken content is clipped or wrapped by display cell without splitting wide Unicode
  characters. Renderer-added whitespace is never mistaken for content indentation.

## Loading, empty, and failure states

These states use the same content origin and participant header:

| State | Ordinary copy |
| --- | --- |
| First page loading | `Loading messages…` |
| Loaded but empty | `No messages yet.` followed by the available compose action |
| Initial load failed | `Messages could not be loaded.` plus the plain retry action |
| Older page available | `Older messages available · PageDown` |
| Older page loading | `Loading older messages…` while retained entries remain visible |
| Older page failed | `Older messages could not be loaded · PageDown retry` |
| Diagnostic Inbox row | Plain diagnostic summary; no fake participant or empty chat transcript |

Stable failure codes and causal evidence remain in technical help. Loading never replaces a
previously complete retained page with a blank pane.

## Semantic theme roles

The implementation should add roles rather than hard-code colors in `render.rs`:

| Role | Meaning |
| --- | --- |
| `conversation.author.self` | `You` author label |
| `conversation.author.participant` | Named or fallback counterparty author label |
| `conversation.activity` | Neutral/running compact activity |
| `conversation.activity.success` | Successful compact activity |
| `conversation.activity.warning` | Interrupted or caution activity |
| `conversation.activity.error` | Failed activity |
| `conversation.selection.focused` | Full-row selected transcript item with transcript focus |
| `conversation.selection.unfocused` | Retained selected item while an inspector or draft owns focus |

Ordinary body text continues to use `ui.text`; technical detail uses `ui.text.technical`. Existing
list selection roles may continue for Inbox rows.

The terminal theme may use distinct ANSI colors for the two author roles, muted activity, and
status colors. Base16 maps self/participant authors to two distinct accent palette entries,
selection to `base02`, neutral activity to `base03`, success to `base0B`, warning to `base0A`, and
error to `base08`. The no-color theme uses explicit labels, bold for authors, dim for neutral
activity, reverse plus weight for focus, and bold status text. Snapshot tests must inspect styles as
well as text so a color regression cannot silently erase hierarchy.

## Typed presentation boundary

The target TUI vocabulary separates navigation context from transcript entries:

```text
Conversation display context
  title
  optional project label
  typed participant mailbox/name evidence

Message entry
  stable fact/action identity
  author: You | Participant(name/fallback) | Unknown
  body
  exceptional visible state
  technical sections

Activity entry
  stable fact identity
  kind
  status
  human summary
  exact detail
  technical sections
```

The concrete structs may differ, but these responsibilities may not collapse back into a single
free-form `summary`. Author classification uses exact conversation context plus sender identity.
`You` is not inferred merely because the sender differs from the named participant unless the
conversation projection proves the closed participant set; unresolved or conflicting evidence
uses an honest fallback. Display labels never become reply, archive, routing, or paging authority.

The local API remains v1 during this pre-release work. Typed activity kind/summary and any author
context can change the existing v1 DTOs in place; no migration, compatibility decoder, or version
bump is required.

Implemented boundary: application snapshots now retain the exact reserved local-human mailbox;
local API v1 carries it with conversation context and carries a closed activity kind on page
entries. The reducer and schema-v1 activity projection retain that kind explicitly. The node maps
exact sender evidence into `You`, the named/fallback participant, or `Unknown sender`, and maps
typed activity kind/status into a generic ordinary summary while retaining the exact original
detail. `UiConversationEntry` is a closed message-or-activity presentation, so action targets and
technical evidence remain independent from display prose. The intermediate renderer now removes
ordinary purpose/ID, `message · open`, `update · information only`, and
`Conversation · complete` labels and begins transcript entries at column zero.

## Implementation migration and acceptance gates

1. Add failing mapper tests for named project/direct participants, local-human authorship,
   unresolved/conflicted identity, typed activity summary/detail, and preservation of exact
   technical evidence. Extend the current v1 DTOs and conversation summary/page context in place.
2. Replace `UiConversationEntry.summary` with typed author/activity presentation. Keep message
   action targets and technical sections separate. Add pure-model tests proving stable selection,
   action capability, paging, resize, reload, and draft focus do not depend on labels.
3. Add a single responsive Inbox pane-width function with boundary tests at the wide threshold,
   list min/preferred/max constraints, and minimum Conversation width. Change the wide 60/40 split
   only through that function.
4. Replace fixed three/six-row entry capacity with measured wrapped spans. Add multiline, long-word,
   Unicode-width, paging, resize, selected-inspector, and draft-constrained viewport tests.
5. Render author/body/activity hierarchy at column zero, add semantic roles to terminal/no-color/
   Base16 mappings and theme documentation, and test every role is explicitly referenced and
   configurable.
6. Move technical entry expansion into the in-pane inspector, preserve exact raw activity detail,
   and gate redundant activity omission on typed kind. Keep failures visible and actionable.
7. Update wide/compact text and style snapshots plus installed PTY coverage using the reported
   conversation shape. Assert absence of `Conversation · complete`, `asynchronous ·`, `project
   output ·`, `message · open`, `update · information only`, and body-leading renderer spaces.
8. Update `docs/rust/tui.md`, `docs/tui-themes.md`, acceptance scenarios, and behavior-ledger
   evidence. Remove obsolete rendering branches and prose only after all semantic action and
   technical-disclosure coverage passes.

## Review decisions

Approval of this specification means agreement on these five product decisions only:

1. **Hierarchy:** messages are full-width author/body transcript blocks at column zero; explicit
   author labels and theme color replace record taxonomy, IDs, bubbles, and indentation.
2. **Widths:** the wide Inbox list is bounded to 24–36 columns with a 32-column preference; all
   additional space belongs to Conversation, which should retain at least 48 columns when possible.
3. **Header:** Conversation is titled by participant with optional project context; it never says
   `Conversation · complete`, and paging appears only when actionable.
4. **Activity:** typed activity becomes a compact status line with exact detail on demand; redundant
   successful turn completion may be hidden only after it has a typed kind.
5. **Focus and details:** full-row theme selection never shifts text; technical evidence moves to
   an in-pane inspector, and no-color retains labels plus non-color focus cues.

The five decisions above are approved. Requested revisions still belong in this document. The
remaining implementation task owns measured layout, theme/accessibility, the inspector, render
snapshots, installed PTY coverage, and final obsolete-presentation removal gates.
