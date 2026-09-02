# HQ

## Next Up

### Make relay sessions wait on socket readiness and exact deadlines

Replace each relay session's healthy `receive_wait`/`recv_timeout` scheduling with an interruptible
event loop over inbound WebSocket readiness, durable-work wakes, policy replacement, shutdown, and
the exact next reconnect/outbound/staging/retained retry deadline. Remove `periodic_poll` and
`receive_wait`; retain capped retry deadlines solely as failure recovery. Test inbound idle wakes,
outbound work, backpressure, disconnect/reconnect baselines, closure, deadline ordering, and orderly
shutdown without advancing a periodic clock.

### Make harness provider events wake the supervisor

Replace `HarnessSession::poll_event` and the component's `event_poll_interval`/`park_timeout` loop
with a mandatory provider-neutral readiness notifier plus nonblocking source-ordered drain. Codex's
reader must signal on queue empty-to-nonempty and terminal closure. Dynamic session launch, bounded
staging/backpressure, responder loss, answer submission, cancellation, and shutdown drain must
neither lose a wake nor busy-spin. Update the neutral adapter contract, conformance suite, fakes,
supervisor, and Codex adapter without coupling the harness to one async runtime. Add deterministic
idle, race, burst, full-queue, provider-closure, and shutdown tests.

### Wake local API sessions when revision invalidations are published

Give `RevisionHub` a coalescing generation/wake observed directly by
`LocalSessionPump::drive_next`. A publication that first makes any active subscriber pending must
wake the pump, and publication racing waiter registration must still be observed. Select this
alongside listener, session, and store readiness, preserve fairness and one pending body-free notice
per subscriber, and never block publishers or let a slow client block commits. Test an idle
Codex-approval-to-wire path, publication/waiter races, coalesced bursts, disconnect/reconnect
baselines, and shutdown without unrelated I/O.

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

* Whenever the conversation view opens, place its selection at the tail and enable follow-tail so
  current and subsequent agent activity auto-scrolls. Restore the same state after approval or
  denial is submitted through the Codex permission dialog.
* <esc> should "pop" the UI back to whatever the containing contextual stack is. So, in the compose note, it should close the compose note and bring you back to the conversation. From there, it should bring focus back to the inbox. From the inbox it should bring you back to the main menu. This <esc> to pop UI affordance should work everywhere.
* The compose area should support both <c-j> and <s-enter> as newline append operators.
* The "Command approval needed" dialog does not properly handle vim navigation (k/j for up/down).
* We should have a Config page that is a sibling to the top nav elements (Inbox...Projects) that allows direct editing of all configuration values. Theme updates should take effect in real time. For theme config, it should show all supported themes. If this feature doesn't exist yet (named themes) then let's plan that out after implementing the Config page scaffolding. Things we should be able to configure: default settings for codex (like yolo, and model selection - for now this can be raw text or nothing for default).
* Let's apply syntax highlighting to the agentic command display. Like, when an agent shows us what shell command it is running conversation view, let's find a good rust library for shell syntax highlighting, and apply that to the presentation of that display element. Ideally we find one that can apply our theme/style color choices. We may have to make a mapping from our color scheme to the various semantic layers of whatever library we choose.
