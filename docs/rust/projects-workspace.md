# Projects workspace

Status: implemented.

## Product boundary

A project owns a body of work, its claimed folders/resources, assignment, lifecycle, and project
conversation links. Git worktrees are an optional folder-provisioning convenience, not the center
of the product. Agents remain independent named workers, and conversations remain first-class in
Inbox. Projects link to those capabilities through typed identities rather than duplicating them.

Ordinary screens use `project`, `folder`, `assigned agent`, `conversation`, and `working folder`.
Exact project IDs, claims, causal evidence, provider/session identities, and operation diagnostics
are available through technical details and recovery.

## Canonical routes

```text
HQ / Projects
HQ / Projects / <project>
HQ / Projects / <project> / Manage
HQ / Projects / <project> / Folders
HQ / Projects / <project> / Folders / <folder>
HQ / Projects / <project> / Technical details
HQ / Projects / <project> / <choice|form|confirmation|progress|outcome|recovery>
```

Project selection at the root is stable list state and does not add path depth. Enter opens the
exact project summary. Manage, Folders, an exact folder, evidence, and workflows are meaningful
children. Escape pops one visible level.

Every route owns the full content pane at every width. There is no persistent list/detail split,
compact preview, pane focus, or centered modal. Compact and wide layouts differ only in wrapping,
clipping, and visible rows.

## Cross-workspace links

A project conversation link installs `HQ / Inbox / <conversation>`. A conversation's project link
installs `HQ / Projects / <project>`. An assigned-agent link installs
`HQ / Agents / <agent>`. These paths are destination-rooted; Back stays inside the destination
workspace and never returns through incidental origin history.

Links carry stable typed identities. Labels, conversation count, recency, assignment, provider,
session, and list position never select a target. Zero project conversations begins guided setup;
one opens the exact row; many opens an explicit typed conversation choice. A not-yet-started setup
retains its own project, agent, provider, and draft identities.

## Project summary and administration

The summary explains the project's current lifecycle, assigned worker, conversation availability,
and folder ownership in ordinary language. It offers only state-valid actions. Administration is
grouped by intention:

- start or continue work in Inbox;
- assign, activate, resume, hand off, or remove an agent;
- inspect, add, check, choose, or remove folders;
- close, reopen, archive, or restore the project;
- inspect technical evidence and recover uncertain operations.

Project mutation effects retain the exact project revision and action/operation identities.
Resource operations retain normalized paths, claim evidence, and check results. Lifecycle and
handoff operations preserve assignment, runtime, release-assessment, and force evidence. A stale
completion may reconcile its exact operation but cannot replace another route or unlock a
different object.

## Workflows

Choices, forms, confirmations, progress, outcomes, and recovery are typed full-pane routes.
Sequential steps replace or consume their current level. Completion returns to the refreshed exact
project, exact folder, or canonical conversation target specified by the workflow; completed forms
and progress screens cannot reappear through Back.

Project creation supports an existing folder and an isolated Git worktree. It validates ownership
before mutation and retains the exact command and derived project identity. A running worktree
operation remains visible and replays only its byte-identical retained request across polling or
reconnect. Completion waits for the exact derived project to appear authoritatively.

Destructive and force decisions name the object, consequence, and recovery implications before
submission. They are full-pane confirmations. Uncertain outcomes retain idempotent reconciliation
evidence; they do not offer an unsafe duplicate retry.

Forms retain text, choice, caret, validation, and target identities across resize, refresh,
reconnect, and recoverable failure. Message text is never entered in Projects: starting or
continuing conversation installs the canonical Inbox route and uses its durable composer.

## State-dependent safety

- Assignment and activation use exact project, agent, provider, session, and thread evidence.
- An agent assigned elsewhere is never taken implicitly; handoff explains and verifies the
  competing assignment.
- Close quiesces active work and evaluates folder release state. Dirty or unknown release requires
  an explicit typed force decision.
- Archive is distinct from close and follows current lifecycle preconditions.
- Folder conflicts name the authoritative owning project when available and never infer ownership
  from display paths alone.
- Pending delivery and response loss reconcile by stable identities. Retry appears only when the
  domain exposes a safe, explicit recovery action.

Invalidation and reconnect preserve the current canonical path while authoritative state is
reread. Reorder retains selection by project/folder identity. Removal falls back to the nearest
valid parent. Old snapshots, checks, and operation results cannot restore stale screens or steal
navigation.

## Canonical journeys

### Create and begin work

1. `HQ / Projects` opens the create choice.
2. Folder or worktree input advances to correlated progress when needed.
3. Completion consumes transient levels and opens the exact new project.
4. Agent choice/creation and provider setup retain typed return context.
5. The first message opens the exact `HQ / Inbox / <conversation>` path.

### Manage an existing project

1. Enter opens `HQ / Projects / <project>`.
2. Manage or Folders pushes one level; exact folder or evidence pushes another.
3. A lifecycle action advances through only the required form, confirmation, progress, and
   recovery states.
4. Success returns to the refreshed exact project; Escape unwinds one still-meaningful level.

### Deep link and reconnect

1. A conversation link installs the exact Inbox path; its Back returns to Inbox.
2. A project or agent link installs that destination's canonical path.
3. Reconnect keeps the path and last coherent presentation.
4. Target deletion falls back by typed route validity, never by matching a name or row position.

## Acceptance

- Every project screen has one typed canonical path or explicit correlated workflow context.
- Every route is full-pane; compact and wide terminals have identical hierarchy and Back behavior.
- Enter opens exact identity-bearing depth or advances a sequential flow; Escape pops one visible
  meaningful level.
- Cross-workspace navigation installs canonical destination paths.
- Project, resource, assignment, runtime, conversation, and operation identities remain distinct
  and authoritative through refresh, reconnect, response loss, and recovery.
- Rendering and resizing do not mutate navigation or workflow state.
