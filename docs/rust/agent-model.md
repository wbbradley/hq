# Named-agent and provider-session reduction

Status: Active implementation contract

`hq_reducer::AgentReducer` is the pure complete-batch policy for mailbox/session bindings,
repository-context history, `FCT-023` through `FCT-026`, and projectless direct sessions. It takes
only canonical facts and explicit `AuthorityPolicy`; it delegates installation-local signer and
scope checks to the authority stage and performs no filesystem, provider, process, lease, presence,
environment, or clock observation.

## Names and lifecycle

An agent name is a nonempty lowercase ASCII slug containing lowercase letters, digits, and internal
hyphens. A claim cites an exact projected agent-mailbox root on the same installation. The reducer
groups claims independently by name, agent ID, and full mailbox address:

- one compatible subject produces an active named-agent identity and permanent reservation;
- incompatible subjects remain explicit candidates and conflict, with no active/runnable agent;
- a retirement cites the exact claim and is absorbing for runnable state; and
- retired names remain reserved forever, so later reuse conflicts or is rejected rather than
  selecting by time, arrival, or fact ID.

`AgentLifecycle` is `Active`, `Conflicted`, or `Retired`. A named agent is runnable only when its
claim axes are active and its selected-session register has one unconflicted candidate. Retirement
retracts runnable selection while claims, names, bindings, contexts, selections, and rename history
remain queryable.

## Immutable provider-session bindings

`SessionIdentity` is the pair `(ProviderId, ProviderSessionId)`. A binding cites the exact agent
mailbox root. One mailbox may retain several distinct sessions. Binding one session identity to
several mailboxes is an explicit conflict with no unique mailbox; equal session text in different
provider namespaces remains independent.

Every binding also produces duplicate-safe `DirectSessionView` history. It may identify a unique
compatible named agent, but a bare binding never invents a name claim, selection, retirement, or
runnable worker. This preserves projectless direct sessions without mixing them with named-agent
lifecycle.

## Context, selection, and rename registers

Repository context is typed, grow-only display/search metadata. `ContextHistoryView` retains every
fact/value and every usable causal maximum. Context never grants authority. A selection must cite
the exact name claim, immutable session binding, and a mailbox-context fact whose value exactly
matches the selected context.

Selection is a causal multivalue register. Concurrent distinct session/context values expose all
maxima, emit `SelectionConflict`, and block runnable state. A later selection resolves the register
only by descending from every maximum. Equal duplicate values do not conflict. A globally
conflicted session binding cannot become active selection.

Per-session display rename is a separate multivalue register. Concurrent unequal name/clear values
remain sorted candidates and emit `RenameConflict`; a descendant of every maximum resolves to one
name or an explicit clear. Rename does not alter durable selection or synthesize runtime state.
Concurrent selection/rename and retirement may all remain historical facts, but retirement wins the
active projection, and facts causally after retirement are invalid.

## Executable evidence

`crates/hq-testkit/tests/agent_reduction.rs` maps and executes `AGT-001` through `AGT-010`.
Grouped cases cover lowercase validation, all claim axes, permanent retired-name reservation,
session rebinding and provider isolation, unnamed direct sessions, context multivalue frontiers,
exact context matching, concurrent and resolved selection/rename registers, duplicate replay,
every arrival order for the selection race, and absorbing retirement with historical sessions.
