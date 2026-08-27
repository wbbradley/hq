# ADR 0003: Rust client and provider workflows

Status: Accepted

Date: 2026-08-26

## Context

The Go CLI, Bubble Tea TUI, embedded agent guidance, harness supervisor, and Codex adapter expose a
large observable surface. The clean-sheet rewrite retains user goals and safety properties, not
Go command parsing, JSON fields, exit codes, screen cells, key bindings, or provider DTOs. Later
packages need a finite first-release workflow inventory so missing UI work cannot be mistaken for
intentional scope reduction.

## Decision

The first complete Rust release provides these client workflows over one reconnecting local client
library:

- Agent messaging: resolve a current provider/session mailbox, ask and wait for an answer, send
  asynchronously, poll ready replies and unsolicited messages, and inspect a known message by
  stable identity without consuming it. Codex, Claude Code, Pi, and an explicit custom session may
  supply mailbox identity; this does not imply managed runtime support for every provider.
- Human messaging: list/filter conversations, inspect mixed message/activity history, answer,
  cancel/archive, restore, send direct root messages or self-notes, see delivery and causal status,
  and use machine-readable output where automation needs it.
- Installation administration: inspect/configure identity, human devices, peer routing, mailbox
  capabilities, relays, synchronization, node lifecycle/readiness, local preferences, repair, and
  actionable diagnostics.
- Named agents and sessions: create/adopt/list/retire names, start a new session, resume an exact
  historical session, select and rename sessions, stop a local worker, preserve repository
  context, and distinguish durable selection from advisory runtime presence.
- Projects: create/list/show, send, activate, reassign or hand off, close/reopen,
  archive/unarchive, inspect health, add/remove/replace/select path resources, provision a Git
  worktree through a recoverable workflow, and observe remote command/result stages.
- Ratatui: open/sent/archived views, deterministic mixed conversation history, activity and
  technical-detail disclosure, reply and new-message composition, durable drafts, archive undo,
  logical scroll/focus preservation, reconnect state, named-agent/session management, and the
  complete project-first compose and lifecycle flows. Exact Go keys, layout percentages, colors,
  and pixel snapshots are not contracts; accessible discoverability and semantic transitions are.
- Agent help: ship concise embedded or installed guidance for ask/send/wait/poll, delivery
  idempotency, incomplete causal history, and sync behavior so an agent need not know transport or
  identity administration.

The first managed provider adapter is Codex. The exact supported Codex app-server baseline is
selected during the Codex adapter package using then-current official schema/documentation and an
installed-binary probe; Go's `0.149.0` pin is scenario evidence only. Codex support includes exact
new/resumed session acknowledgement, stable submission reconciliation, steering/interrupt,
structured non-secret questions and approvals, supported MCP forms/URLs, normalized final output
and activity, process diagnostics, and graceful drain/kill. Provider-specific permissive execution
settings remain explicit local controls and never enter neutral crates or signed state.

Managed Claude Code and Pi adapters, dynamic provider tools, authentication refresh, attestation,
and other provider-specific capabilities are deferred until they can satisfy the neutral
conformance and reconciliation contract. Raw reasoning, token deltas, complete model payloads,
spinners, and secret-marked interactive input are intentionally not persisted.

CLI command names, argument order, JSON representation, prose, and exit codes will be designed
with the local API and clients. The semantic workflows above are mandatory even if several are
presented through a smaller command grammar.

## Consequences

- CLI and TUI work can be checked against a shared workflow inventory rather than Go screenshots
  or parser tests.
- The local API must cover every supported workflow; no client gets a direct SQLite or signer
  escape hatch.
- Ratatui model/effect and render tests prove state transitions and usable layouts without freezing
  Bubble Tea implementation choices.
- Additional managed providers are additive adapters and cannot weaken stable submission,
  reconciliation, secret-handling, or shutdown requirements.
