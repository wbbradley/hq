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

### Create a project from folder improvements

Currently dialogs looks like:

┌ Create project from folder ──────────────────────────────────
│› Path: ~/src/hq│ (required)
│  Choose the existing folder this project should own
│  Will use: /Users/wbbradley/src/hq
│  Name:  (required)
│  Brief:  (optional)
│Ownership preview: this project will claim this folder in HQ.
│Other projects cannot own this folder or overlapping folders.
│HQ will not take over ordinary filesystem or Git maintenance.
│
│Tab/Shift-Tab field · Enter create · Esc cancel

There is a pipe character at/after the cursor as you tab through the editable fields. It's unclear
what that pipe character is for. Also, after a field has text we should not show (required) or
(optional)
