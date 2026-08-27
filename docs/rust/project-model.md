# Project and resource-claim model

This note defines the public pure reduction model for project facts (`FCT-027` through `FCT-048`).
It specializes the causal algebra and historical authority stages; it does not consult the
filesystem, provider runtimes, process state, leases, relay state, or caller environment.

## Identity and home-linear state

A `ProjectCreated` fact permanently fixes the project UUID, home installation, project mailbox,
optional predecessor, initial metadata, desired resources, optional primary resource, and initial
open/closed state. Unequal roots for one project UUID or one home-qualified mailbox are an explicit
identity conflict. No timestamp, fact ID, or arrival order selects a root.

Every later canonical project fact cites its exact previous state through `PreviousState` and cites
the immutable installation root through `ProjectHome`. Human-directed mutations also cite the same
active account authority through `ActiveHuman` and `AccountMembership`. Two otherwise admissible
children of one head form a permanent first-release fork: reduction retains the common head and
reports every sibling. A child of an absent or unusable head remains unresolved, and a transition
whose typed precondition is false is invalid.

The projection is rebuilt by replaying the unique admitted chain. Mutable metadata, desired
resources, primary selection, health observations, lifecycle, assignment, and input sequence are
therefore derived values. The full canonical facts remain permanent history.

## Lifecycle and resources

Stable lifecycle is `open`, `closing`, or `closed`; archive is a separate presentation flag.
Closing stops new dispatch while retaining claims and the current assignment. Closing completes
only after the assignment has ended. Archive requires a closed, unassigned project. Unarchive makes
the project visible but leaves it closed, unassigned, and claim-free. Forced transitions retain a
typed runtime observation but never claim that an external process actually stopped.

Desired resources and active claims are distinct. The first-release `PathResourcePolicy` compares
canonical working-tree locators for equality and ancestor/descendant overlap. Overlap within one
project is allowed. Across open projects on one immutable home, every overlap participant is
reported and all involved projects fail closed for claim/runnable projections. Equal locator text
under different homes is isolated. Adding, replacing, reopening, or selecting a primary resource is
one typed transition; no successful projection exposes a partially changed desired/claim set.

Health is observation only. It may replace the latest typed health value and support notices, but
it cannot open, close, assign, release, archive, or otherwise mutate project authority. Resource
removal and close release only advisory semantic claims and contain no filesystem deletion meaning.

## Assignment, input, dispatch, and output

An assignment epoch moves through configuring, runnable, blocked, and ended facts. A project has at
most one current epoch, and one agent may be current in at most one project per home-reduced fact
set. A global double assignment keeps every participant visible but non-runnable; ending one epoch
retracts the conflict and may restore the remaining unique epoch.

Runnable state cites the exact assignment binding and an immutable conversation root whose typed
message scope names the project mailbox and project UUID. The launch directory is canonical
semantic context, not evidence that a directory exists. Runtime absence never expires an epoch.

`ProjectInputAccepted` assigns the next contiguous positive home sequence to one exact project
message. `ProjectInputDispatched` binds that accepted input at most once to the then-runnable
assignment, agent, provider session, and immutable thread. Stable input, dispatch, and sequence
reuse with unequal semantics is a conflict.

`ProjectOutputRecorded` retains the stable output ID, typed message, originating dispatch,
assignment/provider binding, and thread. Identical retries normalize to one view; changed content or
provenance conflicts. Output causally after its assignment ended is marked late from inactive;
output that preceded the end retains current-at-production attribution. Neither class can change
lifecycle, claims, assignment, or dispatch authority.

## Remote control

Remote project commands occupy a disjoint control projection. An active-device request is queued
and records its digest, target, operation, and expected head, but does not mutate the project. A
home-signed receipt records the canonical head observed by the home. A terminal outcome is either a
typed rejection or a commit citing an admitted canonical project head, plus an optional definite or
uncertain runtime observation. Unequal values for one stable command identity expose a conflict.

Only home-signed canonical project facts advance the project head. Queueing, relay acceptance,
receipt, runtime success, and remote outcome records never substitute for that authority.

## Deterministic run format

The reducer emits typed project/input/dispatch/output/command keys, closed decisions and reasons,
exact direct support, aggregate frontiers, and normalized conflict participants. Global claim and
assignment passes consume only the admitted per-project states. Complete-batch replay therefore has
no dependence on authored clocks or ingestion schedules, and the same fact set reconstructs the
same lifecycle, head, claims, assignment, provenance, late-output, and command-stage views.
