# Projects workspace interaction specification

Status: review draft. This document proposes the product model and interaction contract; it does
not authorize implementation. The nouns, primary action, responsive layout, and contextual action
matrix require explicit user review before an implementation task is queued.

## Product boundary

Projects is a mostly modeless place to find a project, understand whether work can proceed, see who
is responsible, and administer the folders HQ associates with that work. It is not a second Inbox,
a runtime console, or a command launcher. Message reading and writing belong to Inbox. Detailed
provider, causal, and recovery evidence remains available without becoming ordinary navigation.

### Ordinary nouns

| Noun | Meaning in the interface | Boundary |
| --- | --- | --- |
| **Project** | A durable piece of work and the ownership container for its folders. | Projects owns its name, lifecycle, folder ownership, and assigned agent. |
| **Conversation** | The place where the user and an agent collaborate about a project. | Inbox owns conversation lists, history, drafts, replies, and new project conversations. |
| **Agent** | A named worker that may be assigned to one project. | Projects shows who is assigned; Agents owns the agent's identity and saved service sessions. Help may explain “agent” as “named worker,” but the UI should not alternate nouns. |
| **Folder** | The ordinary path on disk that a project owns in HQ. One folder is the default working folder. | Projects owns folder administration. “Resource” appears only in technical disclosure until HQ supports a user-facing non-folder resource. |

The following are technical terms, not ordinary controls: **assignment**, **thread**, **provider
session**, **dispatch**, **operation**, **claim**, **project head**, **resource identity**, and
**input sequence**. They may appear in contextual technical details or exceptional recovery when
their exact identity is necessary. Ordinary copy instead says assigned agent, conversation, agent
service, message delivery, request, folder ownership, version, folder, and pending message.

### Workspace ownership

| Concern | Owning workspace | Projects presentation |
| --- | --- | --- |
| Read, write, reply, or start another project conversation | Inbox | One primary navigation action into a typed project view in Inbox; never a Projects composer. |
| Understand project status and responsibility | Projects | Persistent selection-driven summary. |
| Add, change, remove, select, or inspect a project folder | Projects | Labeled administration subview and pane-owned forms. |
| Assign or change the agent responsible for a project | Projects | Labeled agent card and pane-owned decision flow. Provider/session details are progressive disclosure. |
| Inspect or manage an agent independently | Agents | A link or navigation action to the exact agent, not duplicated agent administration. |
| Retry an accepted message that automatic delivery could not drain | Projects recovery | A contextual `Retry message delivery` action only when typed stalled-delivery evidence exists. |
| Inspect IDs, canonical paths, causal versions, provider/session bindings, or operation failures | Contextual technical details | `?` then `t`, or a recovery disclosure attached to the affected item. |

## Current interaction inventory

`UiProjectModal::Details` owns a complete `UiProject`, selects one resource, renders a subset of
that data, and exposes nearly every project command as an unlabelled letter. That makes a modal
dialog the main project workspace and makes unrelated operations look equally important.

### Data currently carried into Project details

This table is exhaustive for the current `UiProject` value, including fields that the dialog holds
but does not render.

| Current datum | Current presentation | Classification | Proposed destination |
| --- | --- | --- | --- |
| `name` | Heading | Primary project work | Project list and summary heading. |
| `lifecycle` | Open/Closed; `closing` degrades to “Needs attention” | Primary status | Explicit `Open`, `Closing`, or `Closed` state in summary. |
| `archived` | Overrides status with Archived | Destructive lifecycle/presentation | Archived list state and lifecycle actions; separate from closed. |
| `claimable` | “folders available” or ownership attention | Primary status/recovery | Plain folder-ownership summary and a named conflict banner. Never show “claimable.” |
| `project_id` | Short technical ID | Technical evidence | Technical details only; full when width permits. |
| `home` | Held but not rendered | Technical evidence | Technical details only, labeled Home installation. |
| `head` | Short “version” ID | Technical evidence | Technical details and stale-operation recovery only. |
| `input_sequence` | “next message” number | Technical evidence | Remove from ordinary details; technical message-delivery evidence only. |
| `resources[].display_path` | Folder/resource row | Ordinary administration | Folder card and folder-management subview. |
| `resources[].primary` | `primary` prefix | Ordinary administration | `Working folder` label. |
| `resources[].health` | available/not found/cannot open | Primary status/recovery | Plain folder status beside the exact path. |
| `resources[].active_claim` | owned here/needs attention | Primary status/recovery | Plain `Owned by this project` or a named ownership problem. The word claim is technical. |
| `resources[].conflicting_projects` | Count only | Exceptional recovery | Resolve project names in ordinary conflict copy; exact project IDs in technical details. |
| `resources[].resource_id` | Short technical ID | Technical evidence | Folder technical details and exact action targeting only. |
| `resources[].canonical_path` | Wide-only technical line | Technical evidence | Folder technical details; never replace the familiar display path. |
| `assignment` presence | Assigned agent or Unassigned | Primary status | Agent card. |
| `assignment.agent_id` | Short ID instead of a name | Primary relationship plus technical evidence | Resolve and show the authoritative agent name; ID only in technical details. Ambiguity becomes `Unnamed agent` plus a recovery banner, never an ID-as-name. |
| `assignment.phase` and `assignment.runnable` | Setting up/Ready/Blocked | Primary status | Plain `Setting up`, `Ready`, or `Needs attention` in the agent card. |
| `assignment.blocked` | Raw blocked value | Exceptional recovery | Plain actionable explanation; stable value in technical details. |
| `assignment.cardinality_conflicted` | Multiple-agent warning | Exceptional recovery | Prominent named conflict; no normal change-agent action until resolved or an explicit recovery path applies. |
| `assignment.launch_directory` | Wide-only working folder | Ordinary administration | Always show when assigned; relate it to the folder marked `Working folder`. |
| `assignment.provider` | Wide-only service | Technical/advanced choice | Hidden in summary. Show the service name only while choosing how to start work or in technical details. |
| `assignment.session` | Wide-only “conversation” | Technical evidence | Remove from Projects ordinary copy. Inbox owns conversations; provider session stays technical. |
| `assignment.thread_id` | Wide-only thread | Technical evidence | Technical details only. It must not be labeled a conversation. |
| `assignment.assignment_id` | Wide-only technical ID | Technical evidence | Technical details only. |
| `threads[].agent_id/provider/session/thread_id` | Held but not rendered | Technical history | Inbox project-conversation resolution and technical details. Projects must not offer raw session/thread selection during ordinary assignment. |
| `pending_inputs[].message_id/thread_id/sequence` | Held but not rendered; `d` acts on all | Exceptional recovery | Ordinarily summarized as automatic delivery. Exact evidence appears only when a typed stalled state makes retry relevant. |

Project details currently omits the project brief even though creation collects one. The future
summary may show a bounded brief if the authoritative read model exposes it; the implementation
must not synthesize it from messages or IDs.

### Current controls and disposition

| Current key | Actual behavior | Classification | Proposed control and key disposition |
| --- | --- | --- | --- |
| `↑/↓`, `j/k` | Select a resource inside the modal | Ordinary administration | In the project list, select projects. In the folder subview, select folders. Focus decides the object; no modal capture. |
| `Enter` from Projects | Open Project details | Primary navigation | Open/continue project collaboration in Inbox. `l`/Right or Tab enters the adjacent detail pane instead. |
| `a add` | Preview ownership, identify a path, then add a desired resource; optionally make it primary | Ordinary administration | Labeled `Add folder` in Manage project. Remove the direct `a` accelerator so it does not conflict with Inbox archive. |
| `e replace` | Identify a new path and atomically replace the selected resource identity, preserving primary selection when applicable | Ordinary administration | Labeled `Change folder path` on the selected folder. Remove `e`. |
| `x remove` | Remove the selected desired resource and advisory ownership; keep files, worktree, and branch; force is required while assigned | Destructive administration | Labeled `Remove folder`, followed by a bounded confirmation that names the folder and what remains on disk. Remove `x`. |
| `p primary` | Select the default launch resource | Ordinary administration | Labeled `Use as working folder`; inline result, no confirmation modal. Remove `p`. |
| `r check selected` | Freshly inspect one exact folder and report health/release evidence | Ordinary status/recovery | Labeled `Check folder now` beside that folder. Remove `r`, which belongs to reply/continue in Inbox. |
| `R check all` | Freshly inspect every desired folder | Ordinary status/recovery | Labeled `Check all folders` in folder management. Remove `R`. |
| historical `n send instructions` | Opened a project message form | Wrong workspace | Already removed. It must never return; Projects navigates to Inbox for all message writing. |
| `v set up work` | Create an assignment and start/resume a provider runtime for an unassigned project | Primary setup/administration | Labeled `Assign agent` in the agent card. Remove `v`; hide provider/session mechanics unless a meaningful choice is required. |
| `d send pending` | Reconcile and dispatch every accepted pending project input in sequence | Exceptional recovery | Automatic by default. Show `Retry message delivery` only with typed stalled-delivery evidence and a runnable assignment. Remove `d`. |
| `h move agent` | Quiesce the current runtime, end its assignment, and configure another agent; force may be needed | Ordinary administration with exceptional force | Labeled `Change assigned agent`; keep form/progress in the detail pane and use a modal only for explicit takeover risk. Reserve `h` exclusively for one-level Back. |
| `c assess and close` | Inspect folder release state, quiesce runtime, end assignment, release advisory ownership, and close; dirty/unknown state may require force | Destructive lifecycle | Labeled `Close project`, followed by a bounded evidence-and-confirmation dialog. Remove `c` inside details; section-level `c create` remains separately visible. |
| `o reopen` | Validate folders, reacquire advisory ownership, and open one closed unarchived project | Lifecycle administration | Labeled `Reopen project`; inline progress/result. Remove `o`. |
| `z archive/unarchive` | Archive first drives an open project through safe close, then hides the closed unassigned project; unarchive restores visibility but leaves it closed | Destructive lifecycle/presentation | Labeled `Archive project` or `Restore archived project`, with explicit resulting state. Remove `z`. |
| `Esc` | Close the modal and return to the list | Navigation | `h`/Left/Esc moves back one modeless level. Esc in an edited form offers save/discard only when necessary; it does not dismiss the whole workspace. |

No single-letter action from the current shortcut wall survives inside project details. Every
operation remains reachable by a visible object-bearing row in `Manage project`. Contextual help
may document navigation accelerators, but it must not be the only place that names a project
operation.

## Proposed workspace

### Persistent project summary

Selecting a project updates a stable summary containing, in order:

1. Project name, bounded brief when available, and plain lifecycle status.
2. A conversation card with the primary action into Inbox.
3. An assigned-agent card with name and Ready/Setting up/Needs attention status.
4. A folders card showing the working folder first, then other paths and their plain health.
5. A recovery banner only when folder ownership, assignment, delivery, or an operation needs action.
6. A visible `Manage project…` row leading to labeled administration.

Technical IDs and provider/session bindings are absent from this summary. `?` then `t` discloses
them for the selected project, agent relationship, folder, or recovery request.

### Conversation card and primary action

Projects never decides that one of several conversations is “the project conversation.” It uses a
typed project filter in the Inbox workspace:

| Conversation state | Label | Result |
| --- | --- | --- |
| No project conversation; project is open, unarchived, and folder ownership is usable | `Start conversation` | Switch to Inbox with a visible project filter and open a new project draft in the modeless composer. |
| Exactly one nonarchived project conversation | `Continue conversation` | Switch to Inbox, select that exact typed conversation, and focus its conversation pane. |
| More than one nonarchived project conversation | `Open conversations in Inbox` | Switch to Inbox with a visible project filter listing all of them in authoritative reducer order. Do not infer a canonical thread from recency, provider session, or assignment. |
| Only archived conversations, or project cannot currently accept new work | `View conversations in Inbox` | Switch to the filtered Inbox workspace read-only; explain the project state beside any unavailable compose action. |

The filter is navigation context, not conversation identity. It is keyed by stable project ID,
shows `Project: <name>` and a clear action, and never parses a row title. Inbox remains responsible
for snippets, participants, list selection, conversation detail, continue/new-conversation actions,
and drafts.

### Project administration

`Manage project…` replaces the shortcut wall with an in-pane action list. Only applicable actions
are enabled; unavailable actions carry a short reason and do not emit doomed commands.

The action groups are:

- **Folders:** `Add folder`, then a row per folder with `Change folder path`, `Remove folder`, `Use
  as working folder`, and `Check folder now`; `Check all folders` appears when at least two folders
  exist.
- **Agent:** `Assign agent` when unassigned, `Change assigned agent` when assigned, and `Open agent
  details` when the authoritative agent can be resolved.
- **Lifecycle:** exactly the applicable subset of `Close project`, `Reopen project`, `Archive
  project`, and `Restore archived project`.
- **Recovery:** `Retry message delivery`, `Continue closing`, `Retry folder check`, or `Review
  uncertain change` only when typed evidence says that recovery is needed.
- **Technical details:** a single progressively disclosed entry; never a list of IDs in the ordinary
  action menu.

Add/change-folder, assign/change-agent, and recovery progress replace the detail pane with a named
subview. Completion returns to the same project and selected object. Rejection stays in that subview
with inputs intact and a plain next action. A stale target reloads and reselects by typed identity or
reports that the object no longer exists; it never retargets another project, folder, or agent.

### State-dependent action matrix

| Project state | Conversation action | Folder actions | Agent action | Delivery recovery | Lifecycle actions |
| --- | --- | --- | --- | --- | --- |
| Open, claimable, assigned and ready | Start/Continue/Open conversations | Enabled; removing while assigned requires explicit override | `Change assigned agent` | Hidden during normal automatic drain; retry only when typed stalled evidence exists | `Close project`, `Archive project` |
| Open, claimable, unassigned | Start/Continue/Open conversations; messages may wait for assignment | Enabled | `Assign agent` | Hidden until assignment is ready; pending count is status, not a button | `Close project`, `Archive project` |
| Open, assignment setting up | Start/Continue/Open conversations | Enabled except changes invalidated by an in-flight operation | Show setup progress; change only after stable outcome | Automatic; no manual action during ordinary progress | Close/archive disabled with `Finish or cancel setup first` unless the workflow supports safe continuation |
| Open, assignment blocked | Continue/Open conversations and explain that new messages will wait | Enabled if folder ownership is usable | `Resolve assigned agent` or `Change assigned agent`; force is separately confirmed | Retry only after assignment becomes runnable | `Close project` may surface force evidence; `Archive project` follows close |
| Open, folder ownership conflict | `View conversations in Inbox`; new draft disabled | Show conflicting folder/project names; allow non-conflicting corrective folder actions | Existing assignment shown but not runnable | Hidden | `Close project`; archive through close |
| Closing | `View conversations in Inbox` | Read-only while closing | Show agent quiescence status | Hidden | `Continue closing` only when recovery evidence requires it; no reopen/archive until closed |
| Closed, visible | `View conversations in Inbox`; no new project draft | Read-only until reopened | Unassigned | Hidden | `Reopen project`, `Archive project` |
| Archived | `View conversations in Inbox`; no new project draft | Read-only | Unassigned | Hidden | `Restore archived project`; restoration leaves it closed, then `Reopen project` is a separate choice |
| Cardinality or identity conflict | `View conversations in Inbox` without guessing a target | Read-only unless an exact safe correction is available | Name every resolvable participant; ordinary reassignment disabled | Hidden | Only actions proven safe by typed state; technical recovery explains the conflict |
| Operation running | Retain the prior safe navigation action | Affected object locked; unrelated safe objects remain navigable | Affected relationship locked | No duplicate retry | Inline named progress and cancel only if the workflow truly supports cancellation |
| Operation rejected or uncertain | Retain selection and user input | Retry only from exact retained request | Same | Same | Inline plain failure plus `Review technical details`; uncertain effects reconcile by exact operation identity rather than manual duplication |

The implementation needs typed delivery and in-flight/recovery presentation. The mere presence of a
pending input is insufficient to show a retry: accepted input may be waiting normally for
assignment or automatic ordered dispatch.

## Responsive interaction maps

The drawings show hierarchy, not final styling. Vertical dividers have the same focused/unfocused
semantics as the Inbox workspace. There is no outer detail box.

### Wide terminals

```text
 HQ            │ Projects · 4          │ Project · API redesign
 Inbox         │ › API redesign        │ Open · Ready
 Sent          │   open · ready        │ Move authentication to passkeys.
 Archived      │                       │
 Agents        │   Docs refresh        │ Conversation
 Projects      │   closed              │ › Continue conversation
               │                       │   2 conversations in Inbox
               │   Billing cleanup     │
               │   needs attention     │ Agent
               │                       │   Alice · Ready
               │                       │
               │                       │ Folders
               │                       │   Working folder · ~/src/api
               │                       │   ~/src/shared · available
               │                       │
               │                       │ Manage project…
────────────────────────────────────────────────────────────────────────────
 Enter continue conversation · l/→ details · c create · / search · ? help
```

Focus behavior:

- **Navigation focus:** `j/k` changes top-level section; `l`/Right/Enter enters the project list.
- **Project-list focus:** `j/k` changes project and immediately replaces the adjacent summary.
  Enter invokes the selected project's primary conversation action. `l`/Right or Tab enters the
  detail pane. `h`/Left returns one level to navigation.
- **Project-detail focus:** `j/k` moves among the conversation, agent, folders, recovery, and
  `Manage project…` rows. Enter opens the selected row. `h`/Left returns to the project list without
  changing project selection.
- **Administration/form focus:** `j/k` chooses a labeled action or option; Tab/Shift-Tab traverses
  fields; `h`/Left/Esc returns one subview, preserving a safe draft form or asking about unsaved
  changes when required. It never jumps to global navigation.

### Compact terminals

The list and detail are ordinary screens in the Projects section, not overlays.

```text
 Projects · 4

 › API redesign
     open · ready
   Docs refresh
     closed
   Billing cleanup
     needs attention

──────────────────────────────
 Enter continue · l/→ details
```

```text
 Projects / API redesign
 Open · Ready

 › Continue conversation
   Agent · Alice · Ready
   Folders · 2
     Working · ~/src/api
   Manage project…

──────────────────────────────
 Enter choose · h/← Projects
```

On compact screens, Enter from the list still performs the primary conversation action; `l`/Right
opens project detail. `h`/Left from detail returns to the exact selected project in the list, and a
second `h`/Left returns to top-level navigation. Resize preserves project, subview, focused object,
and safe form state by typed identity.

### Forms, progress, and confirmation

- Search is an inline query owned by the project list, not a modal.
- Project creation is a named Projects subview. Guided creation may still be entered from `New…`,
  but its fields, progress, and outcome are not centered modal workflow state.
- Add/change-folder and assign/change-agent are detail subviews with full-width fields and visible
  requirements. Their data survives refresh, resize, recoverable failure, and modeless navigation.
- Routine progress and completion replace or annotate the owning action row. They do not open an
  outcome dialog and do not bounce back into obsolete Project details.
- A modal is permitted only for a bounded destructive or force decision: remove a folder from HQ,
  force removal while assigned, force agent takeover, close with dirty/unknown release evidence,
  archive (including its close consequence), or discard unsaved input. The dialog names the exact
  project/folder/agent and what HQ will keep.

## Scenario walkthroughs

### Fresh creation

Creation finishes on the new project's summary. The conversation card says `Start conversation`.
Activating it switches to the filtered Inbox workspace and opens the modeless project draft. If the
guided path also chooses an agent, ordinary single-service setup remains automatic and the user
lands on the exact committed conversation.

### Active conversation

Selecting the project shows agent and folder status without loading message history into Projects.
Enter moves to the exact sole conversation, or to a project-filtered Inbox list when several exist.
Reply and New conversation remain Inbox actions.

### Unassigned or blocked project

An unassigned project says `No agent assigned` and exposes `Assign agent`; it may accept messages,
which plainly say they will wait. A blocked assignment says what needs attention and keeps exact
technical reason behind help. It never presents `dispatch`, `thread`, or a raw agent ID as the next
step.

### Multiple folders

The summary shows the working folder and a count. Folder management lists every path and its
status. Each selected folder owns `Change folder path`, `Remove folder`, `Use as working folder`,
and `Check folder now`; the all-folders check is separately labeled.

### Ownership conflict

The affected folder names every resolvable competing project and relationship in plain language.
The project remains visible and non-runnable. Corrective folder administration is available only
when its exact semantics are safe; technical details retain canonical paths, resource IDs, project
IDs, and overlap relationship.

### Agent handoff

`Change assigned agent` starts an in-pane choice flow. The ordinary choice is the named target
agent; service and saved-session evidence is shown only if there is more than one meaningful way
to continue. HQ explains that current work must stop before responsibility changes. Failed or
unknown quiescence produces a separate takeover confirmation and never silently revokes authority.

### Pending-delivery recovery

Normal accepted inputs drain automatically and appear only as `Messages waiting while setup
finishes`. A manual retry appears only after typed evidence distinguishes a stalled drain from
ordinary waiting. It says `Retry message delivery`, preserves ordering and idempotency, and shows
exact input/operation identities only in technical details.

### Close and reopen

`Close project` first shows fresh folder release evidence and explains that folders, files,
worktrees, branches, conversations, and history remain. Dirty or unknown state requires an explicit
override. A closed project is visible, unassigned, and claim-free. `Reopen project` validates its
folders and reacquires ownership before returning to Open; failure stays beside that action.

### Archive and restore

`Archive project` explains that an open project must close first and that archive hides rather than
deletes it. It never claims archive is presentation-only when the command will run the close
workflow. `Restore archived project` makes the closed project visible; it does not silently reopen
or reassign it.

### Narrow terminal

Project list, detail, management, and form are separate navigable screens. Every screen has a
visible title and one-level back instruction. No centered Project details rectangle captures the
application, and no required action exists only in a clipped shortcut wall.

## Progressive disclosure and invariants

- Project selection is stable typed identity. Names, statuses, and paths may change without
  changing selection; stale completions may not mutate another project.
- Project summary data comes from authoritative projections. Joining an agent name or conflicting
  project name is display-only; ambiguity degrades to a plain unnamed label and never chooses an
  identity.
- Inbox conversation routing uses typed project/conversation keys. It never parses row IDs, titles,
  provider sessions, or snippets and never guesses which of several conversations is canonical.
- Message composition exists only in Inbox. Projects may navigate to a new Inbox draft but may not
  host, clone, or retain a message editor.
- Folder display paths are ordinary labels; canonical paths, identities, and overlap relations are
  technical evidence. No UI label changes the underlying filesystem.
- Pending dispatch remains automatic and idempotent. Manual retry is recovery, not routine project
  administration.
- Running and uncertain operations retain exact operation identity. Duplicate completion is inert;
  an uncertain external effect is reconciled rather than repeated manually.
- Modal confirmations are bounded decisions, never the main project workspace. Forms, status,
  progress, normal completion, and recoverable failure remain in the owning pane.
- Global navigation meanings win: `h`/Left is Back, Inbox `r` is reply/continue, and Inbox `a` is
  message archive. Project operations use labeled rows rather than repurposing those keys.

## Migration path away from Project details

Implementation should be queued separately after this specification is approved.

1. Add typed project-workspace state and presentation DTOs that separate summary, folder,
   assignment, conversation-count, recovery, and technical evidence. Do not move the complete
   `UiProject` into another monolithic view.
2. Make Projects selection own a replaceable adjacent detail projection, stable object focus, and
   wide/compact navigation state. Add model tests first for selection updates, one-level focus,
   resize/reload preservation, and stale completions.
3. Add typed project-to-Inbox navigation, including zero/one/many conversation behavior and a
   visible clearable project filter. Test exact identity and prove composition remains Inbox-owned.
4. Add the labeled Manage project action catalog and state gates. Route existing command effects
   through pane-owned forms/progress/results; add typed stalled-delivery presentation before
   exposing retry.
5. Restrict modals to the approved destructive/force confirmations and remove routine outcome
   dialogs. Retain exact response-loss and reconciliation behavior in the owning pane.
6. Replace `UiProjectModal::Details`, its selected-resource state, shortcut branches, render block,
   and completion continuations only after model, responsive render, accessibility/text, executor,
   and installed PTY tests cover every old command path.
7. Update TUI and acceptance documentation, then remove obsolete modal snapshots and assert that
   `Project details`, the shortcut wall, `n send instructions`, and dialog-owned project forms do
   not appear in production rendering.

## Review decisions

Approval of this specification means agreement on these four product decisions only:

1. **Nouns:** Project, Conversation, Agent, and Folder are the ordinary model; runtime and causal
   vocabulary is technical disclosure.
2. **Primary action:** Enter from a selected project goes to typed project collaboration in Inbox;
   zero, one, and many conversations have explicit behavior and Projects never guesses a canonical
   conversation.
3. **Layout:** Wide terminals use persistent selection-driven list/detail panes; compact terminals
   use ordinary list/detail screens with one-level Back, never a Project details overlay.
4. **Actions:** The shortcut wall is removed. Full object-bearing actions live in a state-dependent
   Manage project subview; message delivery is automatic and destructive/force confirmation is the
   only routine reason for a modal.

After review, requested revisions belong in this document. A separate implementation task must then
name test-first model, responsive rendering, installed PTY, accessibility/text, documentation, and
complete old-modal removal coverage.
