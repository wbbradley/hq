# Rust node lifecycle and ownership foundation

Status: Active foundation contract

The Rust node is the only composition root. Before any listener, relay, managed runtime, or project
worker starts, one `NodeFoundation` acquires these owners in order:

1. the process-lifetime `node.lock` for the selected state root;
2. the validated private installation identity;
3. the validated unsigned local configuration;
4. the private runtime directory; and
5. the bounded synchronous store actor.

Each step returns only typed redacted failures. An early return drops every earlier owner, so a
corrected startup can immediately reacquire the lock. Normal checked shutdown closes and joins the
store before the runtime, identity, and state-lock owners are dropped. `Drop` repeats the same
idempotent best-effort containment for panic and early-return paths; callers use checked shutdown
when failure reporting matters.

After these foundations open but before any component starts or readiness is published, foreground
startup reconciles the owned identity with canonical authority. An empty clean store authors one
`InstallationDeclared` root through `StoreGateway` and the normal application mutation path. A
nonempty store must project the same installation and derived signing/encryption keys. Restart
reuses the retained mutation and remains at the existing revision; unequal identity/database roots
fail closed. No CLI signer or direct database path participates.

## Runtime namespace

The runtime namespace is distinct from durable identity and storage. `RuntimePaths::new` accepts an
explicit nonempty absolute root. `RuntimePaths::derive` otherwise selects:

- `$XDG_RUNTIME_DIR/hq/<installation qualifier>` when an explicit XDG runtime input is available;
  or
- `<state root>/runtime` as the macOS/service-manager fallback.

The installation qualifier is the lowercase hexadecimal first 96 bits of the installation ID. It
is collision resistance for a non-authoritative local pathname, not an identity or authorization
input; the state lock and later peer/session checks remain authoritative for ownership. A qualifier
collision fails safely at listener binding.

The root is exactly `0700`, may not be a symbolic link, and owns reserved `node.sock` and
`node-ready.v1.json` paths. Reserved artifacts may not be symbolic links. A portable first-release
limit of 103 pathname bytes reserves the terminating NUL in macOS's 104-byte `sockaddr_un.sun_path`
while also fitting Linux. Construction rejects a longer socket path before filesystem or bind work.

Runtime-directory preparation creates or validates the root but never removes a socket, readiness
file, or other stale artifact. Listener binding may replace a stale socket only after the
foundation holds the installation state lock, a nonblocking probe proves that no live listener
responds, and the socket device/inode remains unchanged immediately before removal.

## Lifecycle and admission

The pure lifecycle has five closed phases:

| Phase | Meaning | Status | Queries | Mutations | Launches |
| --- | --- | ---: | ---: | ---: | ---: |
| `Starting` | required owners/components are not yet acknowledged | yes | no | no | no |
| `Ready` | required owners/components are available at a serialized store revision | yes | yes | yes | yes |
| `Draining` | new side effects are closed while accepted work drains | yes | yes | no | no |
| `Failed` | a stable startup/runtime failure is retained | yes | no | no | no |
| `Stopped` | all ownership has been acknowledged closed | no | no | no | no |

Readiness is legal only from `Starting` and records the store's authoritative current revision. A
stop request enters `Draining` with `Stop` intent. A restart request is legal only from `Ready`,
enters the same drain phase with `Restart` intent, and does not itself start a replacement process.
Repeated identical drain, restart, and stopped acknowledgements are idempotent. Out-of-order
readiness/restart/stopped events return a typed lifecycle error.

Startup diagnostics contain a closed component, cause, and suggested operator action plus only the
state/runtime roots explicitly selected by the caller. They do not retain operating-system,
SQLite, key, configuration, or adapter prose. Safe public installation metadata is available
separately; signer secret bytes have no diagnostic path.

## Executable contracts

`crates/hq-node/tests/node_foundation.rs` covers absolute and installation-qualified derivation,
portable length rejection, private modes, symbolic links, non-destructive stale artifacts, every
lifecycle admission transition, out-of-order restart/readiness, concurrent state ownership,
missing identity, unsafe runtime, store-open rollback, mutation rejection during drain, checked
store close, immediate lock reacquisition, and redacted debug surfaces. Foreground CLI contracts
add first-start revision-one bootstrap, restart without a duplicate root, authoritative snapshots
on a fresh installation, and redacted rejection when the persisted root disagrees with identity.

## Component ownership and drain

After foundation startup, `NodeOwner` retains exactly four long-lived component slots in dependency
order: local sessions, relay manager, harness supervisor, and project workflows. Each concrete
owner keeps its existing application capabilities and separately implements the lifecycle seam:
start acknowledgement, stop intake, graceful drain, and idempotent forced stop. A failed start
force-stops the partially started component, rolls earlier components back in reverse, and then
drops the foundation.

The foreground relay slot is concrete. `NodeFoundation::compose_relay` constructs its envelope
codec inside installation-identity ownership, borrows the sole store long enough to issue restricted
relay-state and replication capabilities, and returns a `RelayNodeComponent`; it does not expose a
store getter or root secret bytes for wiring. The component owns one joined `RelayManager`, resolves
routes only from a singular verified authority frontier, revalidates every relay hint, and sends
opened canonical bytes through the shared parse/signature/dispatch/semantic/store-ingest path.
Stop-intake rejects new application relay effects, while drain joins every session owner.

The harness slot is likewise concrete. `HarnessNodeComponent` owns the neutral supervisor and
implements application harness control without exposing provider sessions or storage. It composes
the provider registry, record-only `HarnessStoreAdapter`, normalized persistence capability, and
injected clock/token sources. Exact resume immediately reconciles durable pending/uncertain work.
One component-owned joined thread continuously polls every live worker in bounded zero-wait passes,
normalizes output/activity into the fixed FIFO, and retains one just-polled value per source when
the FIFO backpressures. Stop-intake rejects new launch/control effects and closes provider intake
before stopping that thread; drain continues bounded polling, joins the thread, flushes accepted
events, bounds adapter wait, records escalation, force-stops runtime ownership, and releases exact
worker leases.

Foreground normalized persistence is concrete. `CanonicalHarnessPersistence` derives stable
command identities and complete request digests, then invokes pure application planners through the
same waking canonical mutation path as other local facts. It revalidates the active agent mailbox
and exact direct-session or runnable-project binding inside the transaction snapshot, attaches the
local installation/mailbox/binding and prior activity frontier, and maps stale state, changed
identity, uncertainty, and adapter failure to closed neutral classes. No output body, activity
content, provider diagnostic, or storage error enters its `Debug` or error values.

Managed named-agent control validates the authoritative active claim and unique local agent mailbox
before provider I/O. The node resolves and canonicalizes the absolute launch directory, then passes
only copied environment bytes to the neutral supervisor. After exact readiness it idempotently
authors the immutable mailbox/session binding, repository context, and complete-frontier selection;
an uncertain canonical commit remains uncertain and is replayed under deterministic stage
identities. Foreground composition injects this canonical adapter and is the only production root
that registers the Codex factory. It resolves provider-private executable, model, permissive mode,
developer instructions, and validated working-tree launch policy there. Project activation carries
its selected directory through the passive runtime request before the later canonical runnable
acknowledgement. Restart drains all children and a replacement generation reconstructs the registry
and ownership graph from the retained foundation state.

The foreground project slot is concrete as well. `ProjectNodeComponent` owns bounded startup and
shutdown repair around the saga store, canonical and remote application adapters, shared harness
runtime, read-only path resources, bounded mutating Git adapter, and relay wake scheduling. Store,
harness, and relay owners remain singular: the project component receives only cloneable narrow
capabilities, while component clones share opaque behavior/lifecycle state rather than duplicating
external ownership. Fresh unrepaired projection state defers remote-command scanning without
implicitly repairing it; operational saga repair still runs. Intake is opened only after that
bounded pass, and each accepted command is synchronously checkpointed.

The node owns a hierarchical cancellation root. A child observes cancellation by itself or any
ancestor, but cancelling it cannot affect its parent or siblings. The node also owns a
fixed-capacity task tracker: spawn intake is explicit, every accepted native thread handle remains
tracked, shutdown closes intake and joins all handles, and returned failures and panics become
stable named report entries. A generic nonblocking fixed-capacity mailbox returns the unsent value
with explicit `Full` or `Closed` disposition and never allocates beyond its configured slots.

The future socket runtime is a central node loop rather than a set of tasks that each retain store,
relay, harness, and project owners. Each `ServerSession` owns negotiation, write-ticket, and
subscription state, but borrows a fresh transient application bundle and the lifecycle capability
only while dispatching one decoded request. This preserves the sole component owner and ordered
shutdown boundary without introducing reference-counted capability wrappers for task lifetimes.

Normal component shutdown executes these stages even when an earlier stage reports failure:

1. enter lifecycle drain, closing mutation and launch admission;
2. stop intake for local sessions, relays, harnesses, and project workflows in that order;
3. cancel the node root so every component/task subtree observes shutdown;
4. drain project workflows, provider output/activity, relay durable handoff, and local sessions in
   reverse dependency order;
5. force-stop only a component whose drain failed or explicitly requested escalation;
6. close task spawn intake and join every accepted task; and
7. close the store/foundation and release runtime, identity, and state ownership.

The returned shutdown report lists typed stage/component issues, escalated components, and the task
join report. It is diagnostic evidence, not permission to skip cleanup. `NodeApplicationPorts`
borrows one complete application bundle: query/mutation through `StoreGateway`, revision observation
through `RevisionHub`, and relay, harness, and resource operations through their concrete owners.
The identity internally shares one reference-counted signer handle with the gateway; secret bytes
remain inaccessible and no second signer is constructed.

## Unix listener and readiness artifact ownership

The live `NodeFoundation` is the only public socket bind boundary. Binding rejects symbolic links,
non-sockets, modes other than `0600`, and a path that accepts or is conservatively completing a
connection. A connection-refused socket is stale; it is removed and rebound only while its original
device/inode identity still occupies the path. The retained listener is nonblocking and its created
socket identity remains owned until checked shutdown or best-effort drop.

Every accepted stream is checked before protocol parsing. Linux uses kernel `SO_PEERCRED`; macOS
uses `getpeereid`. The peer user must equal the process effective user. Missing credentials and
mismatches fail closed and drop the accepted descriptor. The safe `nix` wrapper is confined to this
node boundary; no unsafe code or peer-supplied identity enters application/domain policy.

`node-ready.v1.json` is a maximum 4096-byte, canonical, unknown-field-denying JSON diagnostic with
these fields: readiness-metadata version, `ready` lifecycle state, nonzero process ID, bounded build
metadata, nonzero installation ID, acknowledged store revision, and a nonzero boot nonce. The
process ID, build, pathname qualifier, and boot nonce grant no ownership or domain authority.
`NodeOwner` can publish only after its foundation and four components have acknowledged readiness.
Publication creates a unique `0600` same-directory temporary file, writes and syncs the complete
record, atomically renames it, syncs the runtime directory, and retains the installed device/inode.
Reading checks file type, mode, and length before allocating the bounded body, then validates and
canonicalizes every field.

Shutdown closes the listener first and conditionally removes only socket/readiness paths whose
types and device/inode identities still match this owner. Missing files are already clean; renamed,
substituted, linked, or unrelated artifacts are preserved and reported as a typed runtime cleanup
issue. Cleanup continues through store closure and state-lock release even after such an issue.
Asynchronous listener multiplexing and signal coordination remain in the next node package.

## Bounded local session byte I/O

Only a stream returned by foundation-owned same-user credential validation can enter local session
I/O. The raw descriptor is wrapped in an opaque accepted-stream capability and consumed exactly
once when it is registered with Tokio; arbitrary or peer-asserted streams cannot use the production
entry point.

Each connection has one fixed encoded-frame queue and emits into a caller-owned bounded event
channel. Its read half uses the protocol's bounded incremental decoder, drains every retained
complete frame before reading again, and naturally stops socket reads while the event bound is
full. Malformed, oversized, noncanonical, and truncated-at-EOF frames close only that connection.
Its write half preserves queue order and emits a `Written` event for a session-owned ticket only
after every byte of the exact frame succeeds. Invalidation frames are untracked and must already be
the closed invalidation wire variant.

One joined future polls both owned halves. A separate close signal is not queued behind response
capacity; either half terminating drops its sibling and the descriptor before one terminal event is
emitted. Cancelling a write may leave partial bytes at the peer, so cancellation always closes the
connection and never restarts or confirms that frame.

## Bounded local session registry

One central registry owns a fixed number of peer-validated connections. Each admitted connection
has exactly one transport-independent `ServerSession`, one bounded I/O handle, and one joined byte
task. Duplicate connection identities and capacity excess are rejected before spawning. Plain
configuration and drain reports expose their data directly; session and stream internals remain
opaque because they enforce ticket, registration, descriptor, and task-ownership invariants.

Decoded messages borrow the current application and lifecycle capabilities only for that dispatch.
The exact response is queued or the connection is closed; only a matching completed write ticket
advances server state. Protocol errors, malformed input, write failure, queue saturation, and task
failure close only the affected connection. A rejected final protocol version closes after its
typed response has been fully written.

Revision notifications remain coalesced in the shared revision hub. A bounded delivery pass takes
at most one wake per active connection. If the fixed writer queue cannot accept it, the registry
closes that slow connection and cancels its registrations; reconnect performs a full authoritative
refresh, so no retry queue is retained. Drain closes intake, signals every descriptor independently
of queue capacity, consumes terminal events while shared event capacity is saturated, joins every
task, cancels pending and active subscriptions, and reports zero retained sessions and tasks.

## Owned listener and session pump

After binding, the foundation can transfer its listener descriptor exactly once into a Tokio
`AsyncFd`. The opaque transferred listener retains a shared live-descriptor lease, while the
foundation retains the socket pathname and device/inode cleanup identity. Foundation cleanup never
unlinks the socket while that lease is live. Normal pump drain drops the readiness owner first, so
the subsequent foundation cleanup can remove only its exact now-closed socket.

The sole pump selects between listener readiness and registry progress without a polling or accept
task. When both are ready, it alternates preference so sustained connection pressure cannot starve
session dispatch and a busy session cannot starve accept. Readiness is cleared through Tokio's
`try_io` contract only after a nonblocking accept reports `WouldBlock`. Every accepted descriptor is
kernel-validated before the pump derives its connection identity or attempts registry admission.

Connection IDs combine the fresh nonzero boot nonce with a checked nonzero monotonic counter. They
are unique only within that process generation and grant no authority. A full or closed registry
drops the validated descriptor without spawning; closing-session entries continue occupying their
bounded slots until their exact tasks join. Each session progress event also performs one bounded
coalesced-invalidation pass. Explicit pump intake closure drops the listener and closes registry
admission while retaining existing sessions; pump shutdown then joins them all and returns a plain
report before foundation cleanup.
