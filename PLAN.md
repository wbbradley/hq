# HQ

## Next Up

### Represent an unstarted project conversation as a typed setup

The guided project workflow currently stores the chosen project, agent, and provider only in
ephemeral `guided_pending` state while separately installing a synthetic Inbox conversation row
whose `conversation_target` is `None`. Closing the first-message composer discards the typed choice
but leaves the fake conversation and an implicit project filter. The retained surface cannot handle
`r` or Enter, another `n` repeats agent selection, wide list/detail rendering resembles duplicate
conversations, and the footer advertises `Esc clear filter` for a filter the user never chose.

- Introduce one explicit project-conversation setup identity containing the stable project, chosen
  agent, provider, and draft identities. Keep it distinct from authoritative conversation/thread
  rows and persist the setup with the installation-local project draft through the application,
  store, local API, and TUI client boundaries, so Esc, snapshots, reconnects, and process restart
  cannot lose the evidence needed to send and activate the first message. Change current
  pre-release schemas and DTOs coherently rather than adding a compatibility fallback.
- Replace the coupling among `guided_pending`, `pending_project_conversation`,
  `project_filter_rows`, and the `project-draft:<project>` row in `crates/hq-tui/src/model.rs`. An
  unstarted setup must have one stable selectable presentation but must not claim to be an
  authoritative conversation, invent an empty transcript, or depend on display text or row
  position for routing.
- After Esc closes or saves the modeless composer, retain a clear surface such as “Conversation with
  Alice about hq has not started,” state that assignment begins when the first message is sent, and
  advertise `r`/Enter to write that message. Tab must visibly move focus between actual panes rather
  than appearing to select a duplicate. Do not silently install a project filter or render
  `Esc clear filter`.
- Make reopening idempotent. `r` or Enter resumes the exact draft and setup; choosing `n` and the
  same project resumes its existing unfinished setup instead of asking for the same agent or
  creating another item. A deliberate agent change must replace the typed choice explicitly, not
  infer it from the current project assignment.
- On first-message submission, use the retained setup to commit the project input and perform the
  existing activation or handoff exactly once, including after restart or response uncertainty.
  Wait for authoritative root-message/thread evidence, then replace only the matching setup with
  the real conversation; never correlate by prose, timestamp, arrival order, or list position.
- Replace the tests that codify the dead end with pure-model and render coverage for Esc, `r`,
  Enter, `n`, Tab, refresh, reconnect, restart recovery, wide and compact layouts, and absence of
  duplicate-looking rows or hidden filter copy. Extend the installed post-bootstrap scenario
  through project and agent creation, first-message send, activation, authoritative conversation
  replacement, and continuation. Update the TUI, Inbox conversation, and acceptance-scenario design
  documentation to describe the setup state and its modeless shortcuts while retaining modal input
  capture.

Dependencies: the existing project draft persistence and first-input activation/handoff pipeline;
the interrupted Unix-poll fix should land first to stabilize installed verification.

Completion condition: the fresh bootstrap path exposes one intelligible, restart-safe “not
started” setup for the exact project and agent; Esc preserves it, `r`/Enter resumes it, repeated `n`
cannot duplicate it, and sending the first message creates and selects exactly one authoritative
conversation.
