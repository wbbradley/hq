# HQ

## Next Up

### Wake and redraw the TUI directly from model events

Give the TUI executor an OS/event-loop wake source shared by command and observation producers,
then wait on terminal input, executor readiness, and the next real UI deadline together. Drain ready
model events and redraw before sleeping again. Remove the 50 ms terminal sampling bound and
five-minute periodic snapshot refresh; retain manual refresh and explicit retry/autosave/dismiss
deadlines. Shutdown must interrupt and join terminal, client, and observer owners without a polling
deadline. Add deterministic tests from invalidation through pending-interaction refresh to first
dialog draw.

### Add privacy-safe handoff tracing and an installed latency scenario

Add structured boundary records using stable message/fact, dispatch/operation, provider-request,
local connection/subscription-generation, and TUI effect identities plus monotonic receive/emit
instants. Record relay receipt/store commit, project wake/dispatch, Codex submission/provider event,
interaction publication, local invalidation write/read, model update, and first dialog draw without
prompt, message, command, environment, or secret bodies. Installed-daemon diagnostics must be
recoverable for a rerun rather than discarded; records are observability only and never authority.
Add an installed fake-Codex PTY trace asserting ordered boundary records and a small latency budget
attributable only to scheduler/I/O rather than configured polling intervals.

### Document and audit the event-driven interaction pipeline

Update `docs/design.md`, `docs/nostr.md`, `docs/projects.md`,
`docs/harness-contract-v1.md`, `docs/harness-supervisor-v1.md`,
`docs/protocol/local-api-v1.md`, and `docs/rust/tui.md` with the notification graph and the
distinction between event wakes and failure backoff/deadlines. Repository checks must show that no
recurring healthy-state poll interval remains in this pipeline. Verify end to end that a single
local or relayed human message and every resulting provider interaction reach each idle downstream
owner and the next TUI draw without any periodic poll, repair scan, unrelated I/O, or durable
mutation.

### Smaller hand-added todos (each need some in-depth analysis)

* Whenever the conversation view opens, place its selection at the tail and enable follow-tail so current and subsequent agent activity auto-scrolls. Restore the same state after approval or denial is submitted through the Codex permission dialog.
* <esc> should "pop" the UI back to whatever the containing contextual stack is. So, in the compose note, it should close the compose note and bring you back to the conversation. From there, it should bring focus back to the inbox. From the inbox it should bring you back to the main menu. This <esc> to pop UI affordance should work everywhere.
* The compose area should support both <c-j> and <s-enter> as newline append operators.
* The "Command approval needed" dialog does not properly handle vim navigation (k/j for up/down).
* We should have a Config page that is a sibling to the top nav elements (Inbox...Projects) that allows direct editing of all configuration values. Theme updates should take effect in real time. For theme config, it should show all supported themes. If this feature doesn't exist yet (named themes) then let's plan that out after implementing the Config page scaffolding. Things we should be able to configure: default settings for codex (like yolo, and model selection - for now this can be raw text or nothing for default).
* Let's apply syntax highlighting to the agentic command display. Like, when an agent shows us what shell command it is running conversation view, let's find a good rust library for shell syntax highlighting, and apply that to the presentation of that display element. Ideally we find one that can apply our theme/style color choices. We may have to make a mapping from our color scheme to the various semantic layers of whatever library we choose.
