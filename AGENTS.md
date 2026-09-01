Use sound software practices, especially encapsulation, DRY, test-first development, and
decoupling. Preserve typed evidence and stable identities instead of inferring authority from
display text, timestamps, or list positions.

Use Conventional Commits.

HQ is pre-release and has no backward-compatibility requirements yet. Prefer a coherent current
design over compatibility with prior builds.

Design the TUI for people who do not know HQ's internal vocabulary. Screens and dialogs should make
clear what the user is viewing, why input is needed, what actions are available, and what happens
next. Prefer ordinary user intentions over terms such as authority, reducer, provider session,
assignment, thread, and reconciliation. Keep exact technical evidence available through details and
recovery views.

Keep projects, agents, direct messaging, and personal notes distinct and composable. Projects own
work and resources; Git worktree management is an optional convenience rather than the product's
center. Agents are named workers that can be assigned work and contacted through conversations.
Direct messaging is first-class, including future human-to-human communication.

Never require users to guess an identifier, namespace, transition, or recovery command that HQ can
present as a typed choice. Use progressive disclosure: ordinary screens explain goals and next
actions, while detail views expose stable IDs, causal evidence, provider/session identities, and
recovery diagnostics.

Correlate state with typed identities and source sequence, not display prose, timestamps, storage
arrival order, or page-local inference. Treat invalidations as body-free hints to reread
authoritative state; never put prompts, message bodies, secrets, or other sensitive content in an
invalidation.

Keep `PLAN.md` limited to unfinished, task-specific work. Put durable repository guidance here.
Plan entries should state the concrete problem, scope, dependencies, invariants, and observable
completion condition. Do not repeat repository-wide implementation ceremony, generic engineering
advice, or speculative criteria in every task; remove details that no longer affect unfinished
work.

When completing a planned task:

1. Implement test-first and run formatting, strict linting, and relevant tests; run the full suite
   when the change warrants it.
2. Use names based on the capability being built, not a plan position such as “phase 2.”
3. Commit with Conventional Commits.
4. Remove the completed entry from `PLAN.md` and append an implementation summary followed by the
   original task text verbatim to `COMPLETED.md`. Commit that archive update.

`scripts/hq-bootstrap` is a destructive developer helper for repeatedly testing the fresh customer
onboarding journey: it rebuilds HQ, resets local state, and creates a new identity and human account.
