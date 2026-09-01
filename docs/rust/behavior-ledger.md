# Rust behavior ledger

Status: accepted product boundary for the clean-sheet Rust implementation

Date: 2026-08-26

## Purpose and interpretation

This ledger is the complete classification of externally meaningful HQ behavior discovered before
Rust implementation. It separates product and safety outcomes from their Go representations. Later
specifications may refine a `retain` or `redesign` row, but they may not remove its outcome without
an explicit product-boundary decision and a corresponding roadmap change.

Classifications have these exact meanings:

- `retain`: preserve the product, safety, security, algebraic, or operational outcome. Its Go
  spelling, encoding, table, timing, and control flow are not implied.
- `redesign`: keep a desired capability or workflow, but specify its Rust-era contract from first
  principles. Go behavior is scenario and defect evidence only.
- `drop`: intentionally omit the Go behavior or representation.

Release dispositions are `required` for the first complete Rust release, `deferred` for a named
future capability that must not leak into first-release assumptions, and `excluded` for a dropped
behavior. A deferred row is not permission to leave an implicit compatibility path in the first
release.

## Frozen Go baseline

The final Go product baseline is recorded by immutable Git objects:

- Commit: `a2684b21de1d11c2fa0aad2ea3fd83b6c836fe82`
- Tree: `4f18888fc6dc2f82cc315b0b0986a153850a3d01`
- Subject: `fix(syncer): surface node startup failures`
- Authored: `2026-08-26T16:53:52-04:00`

The next commit, `d366b95`, adds only the Rust analysis, governing design, roadmap, and completion
archive. The Go tree at the recorded object is frozen: Rust work may read it, run it in isolation,
or extract reviewed scenario fixtures, but must not develop against it as a compatibility target.
On 2026-08-26, the unchanged Go tree passed `go test ./...` with Go 1.26.5 on macOS ARM64. Opt-in
real-relay and installed-provider smoke tests were not part of that local baseline run and remain
scenario sources for later controlled gates.

No tag is required because the full commit and tree object IDs are the durable baseline. If a Go
oracle defect must ever be characterized, that work occurs separately and cannot silently move
these IDs.

## Evidence inventory

| Source | What was inventoried | Ledger coverage |
| --- | --- | --- |
| `rust-rewrite-design.md` | Governing decisions, retained capabilities, non-goals, invariants, boundaries, protocols, lifecycle, acceptance matrix, and definition of done | Every section; all rows |
| `../crdt-algebra-laws.html` (`crdt-algebra-laws.html`) | Nine laws and concrete conflict policies | `ALG-*`, `REG-*` |
| `rust-port.md` | Risk analysis, compatibility partition, verification, recovery, operability, and cutover | `CMP-*`, `OPS-*`, `REG-*` |
| `rust-port-transcript.md` | User decision history establishing first-principles behavior over Go compatibility | `CMP-*`, ADR links below |
| `README.md` | Human, agent, harness, identity, relay, project, CLI, TUI, delivery, and deployment workflows | `IDN-*`, `MSG-*`, `NET-*`, `RUN-*`, `PRJ-*`, `CLI-*` |
| `docs/design.md` | Node ownership, persistence, mutation, subscriptions, trust, account, activity, and recovery behavior | `ALG-*`, `IDN-*`, `MSG-*`, `NET-*`, `RUN-*`, `OPS-*` |
| `docs/events.md` | Go canonical facts, authority, causal reduction, ordering, answer/cancel, and retention scenarios | `ALG-*`, `IDN-*`, `MSG-*`, `REG-*` |
| `docs/nostr.md` | Addressing, envelope, relay session, inbound trust, delivery, deduplication, and bounds | `NET-*`, `OPS-*`, `REG-*` |
| `docs/lan.md` | Pairing, retained-relay operation, service managers, recovery drills, and security disclosures | `IDN-*`, `NET-*`, `OPS-*` |
| `docs/harnesses.md` | Neutral ownership, provider lifecycle, delivery recovery, events, requests, buffering, and shutdown | `RUN-*`, `MSG-*`, `OPS-*` |
| `docs/projects.md` | Project identity, authority, resources, lifecycle, assignment, dispatch, sagas, clients, and deferred scope | `PRJ-*`, `CLI-*`, `OPS-*` |
| `TUI-Work-thread.md` | Implemented TUI state, activity, drafts, navigation, and reducer-defect evidence | `CLI-*`, `REG-*` |
| `internal/cli/app.go` | Actual command dispatch, global options, subcommands, session discovery, and machine-readable modes | `CLI-*`, `CMP-*` |
| `internal/agenthelp` | Agent ask/send/wait/poll, sync, duplicate-delivery, and incomplete-history guidance | `CLI-*`, `MSG-*`, `NET-*` |
| Go source and tests under `internal/` and `e2e/` | Scenario details, known failure windows, and implementation-specific behavior | Referenced owners; never a Rust oracle by itself |

The command dispatcher, README command summary, every documentation heading, explicit deferred
lists, governing non-goals, acceptance rows, and former roadmap findings were reviewed. Exact Go
constants remain evidence only unless a row below explicitly adopts the underlying product rule.

## Compatibility and representation boundary

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| CMP-001 | retain | required | Add-only signed causal product model | Replicas merge immutable verified facts by set union and reduce them deterministically. | Rewrite design; algebra and reducer packages |
| CMP-002 | retain | required | One owning node per installation | One process owns signer use, store, projections, relay sessions, subscriptions, sagas, and managed runtimes. | Rewrite design; node package |
| CMP-003 | drop | excluded | Go database migration or opening | Rust never opens, migrates, resets, repairs, or writes a Go database. | Rewrite non-goal; identity/store packages |
| CMP-004 | drop | excluded | Go canonical event decoding | Old schemas are unsupported input and have no normal translation path. | Rewrite non-goal; canonical protocol package |
| CMP-005 | drop | excluded | Go local RPC compatibility | Local API v1 is new; Go frames, methods, handshakes, and error shapes are rejected. | Rewrite non-goal; local API package |
| CMP-006 | drop | excluded | Mixed Go/Rust clusters | Go and Rust never operate one installation identity concurrently. | Rewrite non-goal; identity and cutover packages |
| CMP-007 | redesign | required | Rust state, configuration, and runtime paths | Rust derives a separate secure namespace and refuses Go state. | ADR 0001; identity/node packages |
| CMP-008 | redesign | required | CLI grammar and output | Required workflows receive a coherent Rust grammar and typed machine output without Go syntax or JSON compatibility. | ADR 0003; CLI package |
| CMP-009 | drop | excluded | Go CLI exit-code and prose compatibility | Rust maps typed outcomes to documented exits and messages afresh. | Rewrite decision; CLI package |
| CMP-010 | drop | excluded | Bubble Tea screen, key, color, and cell compatibility | Ratatui preserves semantic transitions and usability, not Go rendering details. | Rewrite decision; TUI package |
| CMP-011 | drop | excluded | Go SQLite schema/table/row compatibility | Rust schema is designed by durability class and public query behavior. | Rewrite decision; store package |
| CMP-012 | drop | excluded | Go package and interface boundaries | Rust crates enforce inward dependencies and own protocols explicitly. | Rewrite decision; workspace package |
| CMP-013 | drop | excluded | Go file/config/log formats and paths | Rust versions are new, bounded, secure, and explicitly versioned where durable. | Rewrite decision; identity/node/operations packages |
| CMP-014 | drop | excluded | Go/Rust differential equality as release proof | Specification, laws, Rust batch/incremental equality, and normalized evidence are authoritative. | Rewrite decision; qualification package |
| CMP-015 | redesign | required | Codex provider version compatibility | Select and pin a current Rust-era baseline from official schema and installed evidence. | ADR 0003; `docs/codex-adapter-v1.md`; pinned `hq-codex` schema manifest |
| CMP-016 | redesign | required | Service-manager and release packaging | Provide Rust systemd/launchd guidance for the single executable without preserving Go manifests. | ADR 0001; packaging package |
| CMP-017 | drop | excluded | Go provider sidecar ledger format and location | Rust durable delivery/output state belongs to the new store boundary and has no Go sidecar compatibility. | Harness docs; store/harness packages |
| CMP-018 | drop | excluded | General-purpose CRDT framework | Implement HQ's causal fact algebra and policies directly without claiming a universal framework. | Rewrite non-goal; reducer package |
| CMP-019 | drop | excluded | Universal state-machine/workflow framework | Keep reducer, project, harness, relay, and UI transitions explicit until proven shared laws justify smaller abstractions. | Rewrite non-goal; architecture review |

## Causal algebra, trust, and mutation behavior

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| ALG-001 | retain | required | Join-semilattice merge | Fact-set union is commutative, associative, idempotent, and has the empty identity. | Algebra note law 1; reducer package |
| ALG-002 | retain | required | Input invariance | Reduction ignores fact order, batching, and duplicate delivery. | Algebra note law 2; reducer package |
| ALG-003 | retain | required | Incremental/batch equality | Incremental affected-closure projection equals complete batch reduction exactly. | Algebra note law 3; store/reducer packages |
| ALG-004 | retain | required | Causal dominance | Reachability, never receipt or wall-clock order, permits dominance. | Algebra note law 4; reducer package |
| ALG-005 | retain | required | Exact causal maxima | Every frontier contains all and only maximal usable facts. | Algebra note law 5; reducer package |
| ALG-006 | retain | required | Deferred dependency readiness | Missing or unusable parents defer projection and late parents reconsider reverse dependants. | Algebra note law 6; reducer/store packages |
| ALG-007 | retain | required | Explicit historical authority | Actions cite authority facts at their causal point; current projection is insufficient. | Algebra note law 7; authority package |
| ALG-008 | retain | required | Retractable projections over monotone facts | Facts remain immutable while active grants, visibility, membership, and routing may retract. | Algebra note law 8; reducer package |
| ALG-009 | retain | required | Deterministic conflict policy | Each concurrent domain conflict has a named rule with no hidden last-write-wins fallback. | Algebra note law 9; semantic catalog package |
| ALG-010 | retain | required | Remove-wins safety policy | Concurrent revoke/remove, archive, rejection, and retirement win where their domain rules say so. | Algebra note concrete policies; domain reducers |
| ALG-011 | retain | required | Restore-after-archive policy | A causally later restore may reopen while a concurrent restore loses to archive. | Algebra note; conversation/project reducers |
| ALG-012 | retain | required | Independent answer and cancellation facts | Answers accumulate; a thread may be both answered and cancelled with their causal relation exposed. | `docs/events.md`; conversation reducer |
| ALG-013 | retain | required | Unique-root conflict visibility | Conflicting unique roots remain explicit instead of choosing by timestamp or identifier. | Algebra note; authority/project reducers |
| ALG-014 | redesign | required | Trust transition types | Raw bytes, parsed input, cryptographically verified input, semantic facts, and reduction decisions are distinct types. | Rewrite design; domain/protocol/reducer packages |
| ALG-015 | redesign | required | Common local/remote commit engine | Locally authored and remotely verified facts use one atomic append/reduce/project path. | `docs/design.md`; store package |
| ALG-016 | redesign | required | Stable mutation reconciliation | Same ID and request returns the original result; changed input under one ID is a conflict. | `docs/design.md`; store/application/local API packages |
| ALG-017 | redesign | required | Atomic fact-backed mutation | Canonical append, dependency data, projections, outbox, receipt, revision, and commit are all old or all new. | Rewrite invariant; store package |
| ALG-018 | redesign | required | Complete repair oracle | Repair discards rebuildable state and deterministically reproduces every public projection. | Rewrite design; store/qualification packages |

## Identity, peers, mailboxes, and human accounts

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| IDN-001 | redesign | required | Installation initialization | Generate one stable UUID and root key with secure atomic persistence and explicit randomness; first foreground startup authors exactly one matching installation root before readiness. | `docs/rust/identity-persistence.md`; identity/node CLI tests |
| IDN-002 | retain | required | Public identity inspection | Show installation UUID and public key without exposing the secret. | ADR 0002; offline identity CLI tests |
| IDN-003 | redesign | required | Encrypted identity export/import | Preserve UUID and root authority in a new guarded Rust backup package using explicit secret stdin and no overwrite. | ADR 0002; identity persistence and real CLI round-trip tests |
| IDN-004 | drop | excluded | Routine recursive identity reset command | Identity retirement/removal belongs to an explicit operator archival procedure. | ADR 0002; cutover package |
| IDN-005 | retain | required | Secret exclusion | Root secrets never enter SQLite, facts, RPC results, logs, diagnostics, or ordinary crash reports. | Rewrite invariant; identity/security packages |
| IDN-006 | retain | required | Duplicate identity prohibition | Refuse overwrite and concurrent local ownership; warn against multiple active restored hosts. | `docs/design.md`; identity/node packages |
| IDN-007 | redesign | required | Installation-local configuration | Relay and provider defaults are typed unsigned local configuration, not signed domain state; passive fields are public and persistence revalidates them. | `docs/rust/identity-persistence.md`; identity/config CLI tests |
| IDN-008 | retain | required | Full mailbox address | Routing and authority use installation plus mailbox identity; a bare mailbox ID grants nothing. | `docs/events.md`; domain package |
| IDN-009 | redesign | required | Directional peer binding | Bind installation UUID, root key, label, and relay hints for routing without granting mailbox authority. | `docs/design.md`; authority package |
| IDN-010 | retain | required | Explicit mailbox access grant/revoke/observation | Owner-signed capabilities and receiver observations preserve seen history and fail closed for concurrent/later use. | `docs/events.md`; authority package |
| IDN-011 | redesign | required | Multi-device human account | Creator grants, target accepts, devices fan out account facts and messages, and creator revokes. | README and `docs/design.md`; authority/application packages |
| IDN-012 | retain | required | Membership maximal frontier | All causal-maximal acceptances are tracked; regrant authority must descend from the revoke. | Known defect; authority package |
| IDN-013 | redesign | required | Pairing bundle | A bounded signed bundle carries sufficient historical account authority and relay hints for offline verification. | `docs/lan.md`; protocol/application packages |
| IDN-014 | retain | required | Creator-only account administration | The creator is the sole first-release grant/revoke administrator. | README; authority package |
| IDN-015 | redesign | deferred | Account administration transfer | A future signed protocol may transfer creator authority; first release rejects it. | README deferred scope; future product package |
| IDN-016 | redesign | deferred | Root-key rotation | A future fact/protocol may rotate keys; first release has one root key. | `docs/events.md`; future identity package |
| IDN-017 | drop | excluded | Hostname, IP, port, or relay-presence identity | Network location and presence never prove identity or authority. | `docs/lan.md`; authority/relay packages |
| IDN-018 | drop | excluded | Same-user process security boundary | Permissions reduce accidents/other-user access but do not claim protection from the same OS user. | `docs/design.md`; security documentation |
| IDN-019 | redesign | required | Local peer blocking | Stop new transport only after deliverable revocation work while retaining previously authorized history. | `docs/events.md`; authority/application packages |
| IDN-020 | redesign | required | Default human-account selection | Installation-local signed selection cites that installation's active membership and does not grant membership. Creator bootstrap/show/select reconcile through authoritative local-API snapshots and pure plans. | `docs/events.md`; application planners and real CLI race/restart tests |

## Messaging, conversations, activity, and delivery

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| MSG-001 | redesign | required | Questions and answers | Typed facts support request/response without parsing message prose. | README and `docs/events.md`; conversation package |
| MSG-002 | redesign | required | Asynchronous messages | Send durable content without waiting and later receive replies or unsolicited messages. | README and agent help; conversation/application packages |
| MSG-003 | retain | required | Multiple answers | Valid answers accumulate and waiting consumes the first locally ready answer in canonical order. | `docs/events.md`; conversation/application packages |
| MSG-004 | retain | required | Cancellation independent of answers | Cancellation never erases a valid answer and exposes before/after/concurrent relation. | `docs/events.md`; conversation reducer |
| MSG-005 | redesign | required | Archive, restore, and reject state | Signed state transitions follow explicit causal conflict rules and may retract views. | Rewrite design; conversation reducer |
| MSG-006 | redesign | required | Typed presentation and correlation | Kind, provider/session/operation/item/request identity, action grouping, local-human context, and participant authorship are typed fields, never body parsing. | `docs/design.md`; domain/conversation packages; local API and node/TUI author/activity mapping tests |
| MSG-007 | redesign | required | Ordered technical disclosure | Bounded namespaced technical sections and exact activity detail are display diagnostics and never authority, routing, or ordinary-summary input. Wide views use an in-pane inspector; compact views use a secondary pane that returns to the stable anchor. | `docs/rust/tui.md`; typed TUI routing/semantics/evidence/activity mapping and render tests |
| MSG-008 | retain | required | One canonical conversation comparator | Reducer-owned parent-first deterministic order covers equal times and mixed messages/activity. | Known defect; reducer package |
| MSG-009 | redesign | required | Unified conversation entries | Queries expose a typed message/activity union in canonical order with stable event identity. | `docs/rust/conversation-model.md`; reducer/store page tests and TUI page mapping/model/render tests |
| MSG-010 | retain | required | Activity is non-actionable | Activity cannot create inbox/unread work or become a reply, archive, draft, delivery, or final-answer target. | `docs/rust/tui.md`; typed TUI activity entry and non-actionable render/model tests |
| MSG-011 | redesign | required | Activity coalescing and retention | Stable canonical keys select snapshots/progress winners while terminal/completed items remain history. | `docs/rust/conversation-model.md`; reducer retention tests; TUI consumes the resulting page without selection logic |
| MSG-023 | redesign | required | One typed live activity tail | Ordinary pages hide retained progress/running-turn telemetry; the indexed query selects the latest useful progress for a fully correlated running operation and terminal evidence replaces it once. | Store paging/reopen tests; node/TUI anchor and render tests |
| MSG-024 | redesign | required | Typed completed work | Commands, file changes, tools, and web searches retain bounded structured presentation fields; terminal-safe previews never parse flattened activity prose. | Codex normalization, protocol/store round trips, and node/TUI preview tests |
| MSG-025 | redesign | required | Immediate truthful local submission | The TUI paints one identity-anchored `Pending` human row with the send effect, replaces it from canonical identity, restores exact drafts on definite rejection, and retains one row under uncertainty. | `docs/rust/tui.md`; pure model, executor, and installed PTY tests |
| MSG-012 | retain | required | Output before activity | A normalized provider item persists canonical output before its related activity and reconciles partial success. | `docs/harness-supervisor-v1.md`; supervisor partial-checkpoint recovery test; application planners and node canonical persistence tests |
| MSG-013 | retain | required | Incomplete causal-history disclosure | Directly inspectable addressed content with absent history is marked incomplete and cannot prematurely support projection. | Agent help and `docs/events.md`; query/application packages |
| MSG-014 | redesign | required | Delivery state vocabulary | Distinguish durable queued, relay accepted/rejected, and peer received; relay acceptance is not peer receipt. | README and `docs/nostr.md`; store/relay/query packages |
| MSG-015 | retain | required | Consumer delivery reconciliation | Leased stdout/provider delivery may repeat across the final crash window; stable message identity enables idempotency. | Agent help and harness docs; application/harness packages |
| MSG-016 | redesign | required | Durable local drafts | Installation-local drafts autosave, survive reload/restart, retain stale targets, and submit atomically without replication. | README and TUI work; store/local API/TUI packages |
| MSG-017 | retain | required | Human account fanout | One canonical account-addressed fact creates recipient work for every other active device and is re-authorized on receipt. | `docs/design.md`; authority/store/relay packages |
| MSG-018 | drop | excluded | Public messaging and generic Nostr DMs | HQ supports only defined private/account audiences and does not import kind-14/public events. | Rewrite non-goal and `docs/nostr.md`; protocol package |
| MSG-019 | drop | excluded | Behavioral parsing of body/details/log prose | Human text and diagnostics never determine identity, correlation, presentation, authority, or transitions. | Rewrite security tests; architecture package |
| MSG-020 | drop | excluded | Dense stored global conversation rank | Ordering is reducer-derived or indexed by a stable comparator/cursor. | `docs/events.md`; reducer/store packages |
| MSG-021 | drop | excluded | Legacy message-only conversation-history endpoint | Rust exposes the normalized conversation/message queries its clients need without retaining a legacy Go endpoint. | `docs/design.md`; application/local API packages |
| MSG-022 | drop | excluded | Legacy standalone activity-list endpoint and Go caps | Rust clients consume unified typed pages; projection retention gets a reviewed Rust-era budget. | `docs/events.md`; activity/store packages |

## Relay transport and replication

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| NET-001 | redesign | required | Canonical fact protocol | Canonical fact v1 has strict deterministic encoding, bounds, signatures, versions, and explicit DTO conversion. | Rewrite design; protocol package |
| NET-002 | redesign | required | Independent encrypted envelope protocol | Envelope v1 has its own version, recipient binding, NIP-44 encryption, NIP-59 wrapping, and trust transitions. | Rewrite design; relay package |
| NET-003 | retain | required | Exact wrapper retry lineage | Persist the exact wrapper before first publish and reuse its bytes/key/timestamp for retries. | `docs/nostr.md`; relay/store packages |
| NET-004 | retain | required | Relay metadata has no domain authority | Relay timestamps, order, acceptance, URLs, and observations cannot influence reduction. | Rewrite invariant; relay/reducer architecture tests |
| NET-005 | redesign | required | One owner per relay session | One session handles catch-up, live subscription, auth, outbound attempts, backoff, and lifecycle. | `docs/nostr.md`; relay package |
| NET-006 | retain | required | Retained catch-up with overlap | Offline recovery pages with overlap and deduplicates wrappers/logical facts across restart and duplication. | `docs/nostr.md`; relay package |
| NET-007 | retain | required | NIP-42 authentication and identity agreement | Auth, seal signer, envelope origin, canonical signer, installation, and recipient must agree. | `docs/nostr.md`; relay/protocol packages |
| NET-008 | redesign | required | Staging and bounded quarantine | Transient failures retry; permanently bad input is bounded, diagnosable, and has no projection effect. | `docs/design.md`; relay/store packages |
| NET-009 | retain | required | Non-disruptive work wakes | Ordinary publish/config work coalesces without restarting a healthy live subscription. | Known defect; relay package |
| NET-010 | redesign | required | Relay configuration and status | Typed local configuration controls read/write/auth policy; status exposes actionable health without secrets. | README; application/relay/CLI/TUI packages |
| NET-011 | redesign | required | Explicit synchronization request | A client may request prompt node work, but local commit success is independent and this is not an offline mode. | Agent help and `docs/nostr.md`; application package |
| NET-012 | retain | required | Replica convergence | Distinct Rust installations converge after arbitrary ordering, duplicates, downtime, and retained catch-up. | Rewrite acceptance matrix; relay qualification |
| NET-013 | redesign | required | Controlled real-relay smoke | Correctness uses scripted relays; one pinned controlled relay proves integration and operations. | `docs/lan.md`; qualification package |
| NET-014 | drop | excluded | Relay-host identity from plaintext transport | Plain relay location never authenticates application facts; insecure development transport is explicitly disclosed. | `docs/lan.md`; relay/security packages |
| NET-015 | drop | excluded | Generic relay list publication and forward-secrecy claim | No kind-10050 publication and no claim that NIP-44 offers forward secrecy. | `docs/nostr.md`; security documentation |

## Named agents, harnesses, and provider sessions

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| RUN-001 | redesign | required | Durable named agents | Lowercase installation-local names bind/adopt mailboxes, remain reserved after retirement, and derive from facts. | README; agent reducer/application packages; `hq-projects` coordinator/canonical tests; `hq-node` foreground retirement/restart E2E |
| RUN-002 | retain | required | Durable session versus runtime presence | Selection/history survive restarts; processes, leases, phases, and caller environment remain local operational state. | `docs/harnesses.md`; supervisor recovery and harness storage contract tests |
| RUN-003 | redesign | required | Provider-neutral harness contract | Logical instances, sessions, capabilities, outcomes, lookup, requests, output, cancellation, and shutdown use neutral types. | `docs/harnesses.md`; harness package |
| RUN-004 | retain | required | Safe provider registration | Reject an adapter without stable-ID idempotency or lookup/reconciliation. | `docs/harnesses.md`; harness conformance package |
| RUN-005 | redesign | required | Scripted fake provider | A deterministic adapter exercises new/resumed sessions, loss, races, requests, output, crashes, and teardown. | Rewrite roadmap; harness/testkit packages |
| RUN-006 | redesign | required | Codex adapter | Privately implement the selected baseline's handshake, session, turn, request, event, transport, and process behavior. | ADR 0003; `docs/codex-adapter-v1.md`; `hq-codex` characterization/conformance tests; foreground node registry tests |
| RUN-007 | retain | required | Exact session readiness | Select only after acknowledged start/resume with a matching nonempty durable ID; never silently replace a missing resume. | `docs/harnesses.md`; application binding/context/selection planner tests; Codex exact-resume and node launch-policy tests |
| RUN-008 | retain | required | Stable provider submissions | Persist pending/uncertain/accepted states and reconcile authoritative provider history before retry. | `docs/harnesses.md`; harness/store packages |
| RUN-009 | retain | required | One logical worker owner | Leases/tokens prevent duplicate named-agent workers while allowing crash expiry and exact-owner revival. | README; supervisor recovery and harness storage contract tests |
| RUN-010 | retain | required | Durable automatic work reconciliation | Startup, invalidations, and repair scans wake runnable offline workers without relying on client wake timing. | Completed Go finding; harness/node packages |
| RUN-011 | retain | required | Bounded FIFO plus keyed coalescing | Durable events backpressure; replaceable snapshots coalesce at the tail without reordering intervening work. | `docs/harness-supervisor-v1.md`; live supervisor saturation/staging/restart tests; component-owned poll-task test; node canonical persistence adapter |
| RUN-012 | retain | required | Structured interactive requests | Non-secret questions, approvals, permission scopes, and supported MCP forms/URLs receive exactly one validated human response, or an immediate fail-closed response when no session-owned responder exists. | `docs/codex-adapter-v1.md`; supervisor responder-loss tests; local API acknowledgement/disconnect races; reconnecting-client operations wakes; fake-Codex installed-TUI approval round trip |
| RUN-013 | retain | required | Secret input rejection | Secret-marked prompts/labels/options/answers are not persisted; fail closed with generic diagnostics. | README; harness/security packages |
| RUN-014 | retain | required | Environment copy/redaction | Caller environment is copied only at the control boundary, retained only in documented memory, wiped, and never serialized/logged by HQ. | `docs/design.md`; application launch-environment tests; local API protocol/reconnect tests; harness security and Codex process-environment tests |
| RUN-015 | retain | required | Graceful provider drain and kill | Stop intake, cancel waits, drain accepted work, checkpoint uncertainty, close, escalate kill, wait, and release ownership. | `docs/harness-supervisor-v1.md`; live provider closure/failure tests; component poll-task join test; Codex conformance teardown; real foreground restart-generation test |
| RUN-016 | redesign | required | Typed provider failure causes | EOF, malformed/oversized protocol, child exit, write/read failure, unsupported request, and drain failure remain distinguishable. | `docs/codex-adapter-v1.md`; neutral harness error classes; `hq-codex` transport/process and diagnostic-redaction tests |
| RUN-017 | redesign | required | Session history and naming | Preserve scoped bindings, selection, repository context, launch directory, mutable display names, retirement, and namespace isolation. | README; application session planners; node canonical adapter; agent reducer/store packages; project handoff/retirement tests |
| RUN-018 | redesign | deferred | Managed Claude Code and Pi adapters | Add only after neutral capability and reconciliation conformance passes. | ADR 0003; future provider packages |
| RUN-019 | drop | excluded | Raw reasoning, deltas, spinners, and full provider payload persistence | Normalize only bounded user-facing output/activity and typed diagnostics. | `docs/harnesses.md`; Codex package |
| RUN-020 | drop | excluded | Neutral crates containing Codex vocabulary | Provider DTOs, method names, and options remain private to the adapter/composition root. | Rewrite architecture; dependency tests |
| RUN-021 | retain | required | Additive notification tolerance and blocking-request fail-closed policy | Ignore unknown additive notifications, reject unsupported authority-bearing server requests, and terminate with a typed compatibility cause. | `docs/codex-adapter-v1.md`; pinned fixture and real-adapter conformance tests |
| RUN-022 | redesign | deferred | Dynamic tools, provider authentication refresh, attestation, and time lookup | Add provider capabilities only with an explicit security contract and neutral capability representation. | `docs/harnesses.md`; future provider packages |

## Projects and path resources

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| PRJ-001 | redesign | required | Project identity and immutable home | Each project has a UUID, home authority, mailbox, optional predecessor, mutable metadata/brief, and permanent history. | `docs/projects.md`; project reducer |
| PRJ-002 | retain | required | Sole home mutation authority | Home issues linear state and dispatch facts; replicas never assume authority if it is lost. | `docs/projects.md`; project reducer/remote control |
| PRJ-003 | retain | required | Expected-head compare-and-swap | Every post-create state command cites the expected content-addressed head and stale commands fail with current state. | `docs/projects.md`; project application/protocol |
| PRJ-004 | redesign | required | Desired resources versus active claims | Durable desired membership, active claim epochs, health observations, and release assessments are distinct. | `hq-testkit::project_reduction`; `hq-projects` open/resource workflow tests; project store and `hq-resources` release tests |
| PRJ-005 | retain | required | Assignment cardinality | One current agent per project and one current project per agent; idle means unassigned. | `docs/projects.md`; project reducer |
| PRJ-006 | retain | required | Thread scope immutability | Direct and project threads cannot move between agents, projects, or scopes. | `docs/projects.md`; agent/project reducer |
| PRJ-007 | redesign | required | Reversible lifecycle | Open/closed plus operational preparing/closing and constrained archive state follow explicit transitions. | `docs/projects.md`; `hq-projects` close/archive workflow and codec tests; project reducer |
| PRJ-008 | retain | required | Claims survive runtime absence | Project claims/assignments do not expire with node, process, lease, or machine downtime. | `docs/projects.md`; project store/node |
| PRJ-009 | retain | required | Close/archive do not touch resources | Releasing advisory claims never deletes/modifies files, worktrees, branches, or containers. | `docs/projects.md`; `hq-projects` read-only release port and close/archive workflow tests |
| PRJ-010 | redesign | required | Path identity and conflict | Home-qualified absolute canonical locators detect equal/ancestor/descendant conflicts while preserving human spelling. | `docs/path-resources-v1.md`; `hq-resources/tests/path_identity.rs`; canonical project mutation policy |
| PRJ-011 | retain | required | Missing-path and symlink revalidation | Reserve through nearest existing ancestor and fail/warn on later identity disagreement instead of silently changing it. | `docs/path-resources-v1.md`; missing/symlink/inaccessible fixtures |
| PRJ-012 | redesign | required | Resource health and release assessment | Typed health plus clean/dirty/unknown/not-applicable observations inform workflows without mutating lifecycle automatically. | `hq-resources/tests/path_identity.rs` and `release.rs`; `hq-projects` release/force workflow tests |
| PRJ-013 | redesign | required | Primary path and launch override | Defaults are explicit; outside-claim overrides warn, and invalid resumed directories require a human decision. | `hq-resources::select_primary` and launch validation tests; harness consumer remains downstream |
| PRJ-014 | redesign | required | Activation saga | Open/assign/select/start/send crosses durable checkpoints and compensates to the prior documented stable state on failure. | `docs/projects.md`; project workflow package |
| PRJ-015 | redesign | required | Handoff and forced takeover | Graceful handoff requires quiescence; explicit force revokes HQ authority while disclosing possible external access. | `docs/projects.md`; `hq-projects` handoff/retirement block-force, stale-head, response-loss, and restart-repair tests |
| PRJ-016 | retain | required | Home sequencing and dispatch attribution | Accepted inputs receive home order; dispatch binds assignment/agent/thread and happens at most once. | `docs/projects.md`; project reducer/harness |
| PRJ-017 | retain | required | Late output classification | Preserve attributable history from inactive assignments without granting current authority or masquerading as current output. | `docs/projects.md`; project reducer late-output tests; `hq-projects` handoff/retirement history-preservation tests |
| PRJ-018 | redesign | required | Remote project control | Active human devices queue authenticated commands/results to the home and expose accepted/received/committed/rejected/runtime stages. | `docs/projects.md`; remote-control/application packages; project-catalog projection/render fixtures |
| PRJ-019 | redesign | required | Recoverable Git worktree provisioning | Reserve destination, reconcile Git creation, create project, and compensate/retry with one stable operation ID. | `docs/projects.md`; `hq-projects` workflow/fake-port/real-Git tests; project saga store reservation tests; concrete node project worker |
| PRJ-020 | retain | required | Project-first composition | New work selects/creates a project; direct named-agent messaging remains a separate control plane. | `docs/projects.md`; CLI/TUI/application packages |
| PRJ-021 | redesign | deferred | Project-message cancellation | First release rejects it explicitly; later semantics require their own causal rule. | `docs/projects.md` deferred list; future project package |
| PRJ-022 | redesign | deferred | Generic resource kinds and continuous health polling | Path is the only first-release kind; adapters may add kinds after defining policy and recovery. | `docs/projects.md`; future resource packages |
| PRJ-023 | redesign | deferred | Rich model-visible direct-agent tools and scheduling | Direct mailbox messaging remains; broader agent coordination tools await a specification. | `docs/projects.md`; future application package |
| PRJ-024 | drop | excluded | Project re-homing or remote agent execution | V1 home and runtime co-location are permanent; successor projects handle a lost home. | Rewrite non-goal and `docs/projects.md`; project reducer |
| PRJ-025 | drop | excluded | Project deletion and automatic worktree/branch cleanup | History is permanent and external resources require explicit operator action. | `docs/projects.md`; project workflow |
| PRJ-026 | drop | excluded | Automatic full-history prompt injection or generated handoff summaries | Deliver pending inputs individually; richer summaries require a future explicit feature. | `docs/projects.md`; harness/project packages |
| PRJ-027 | redesign | deferred | Per-project device ACLs or per-agent end-to-end keys | First release uses active human-account device distribution and installation encryption; narrower scopes need a new authority protocol. | `docs/projects.md`; future authority package |
| PRJ-028 | redesign | deferred | Branching or merging project lineage | First release supports one optional predecessor link; richer lineage needs explicit deterministic semantics. | `docs/projects.md`; future project package |
| PRJ-029 | redesign | required | Mailbox receipt independent of runtime delivery | A committed project message returns its durable receipt before background sequencing, dispatch, runtime start/resume/steer, or provider acceptance; coalesced wakes and startup/periodic/drain repair reread durable state. | `docs/projects.md`; project component blocked-worker/coalescing/retry tests and installed PTY |

## Local API, CLI, Ratatui, and user workflows

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| CLI-001 | redesign | required | Reconnecting local API | Strict framed v1 negotiation, build metadata, typed requests/results/errors, reconnect, and full-snapshot recovery. | `docs/rust/cli.md`; `hq-local-api` reconnecting-client state-yield and blocking-runner tests; real subscribed invalidation/restart test in `hq-node/tests/unix_node_cli.rs` |
| CLI-002 | retain | required | Subscription revision-race closure | Register before reading acknowledged revision and activate after acknowledgement is written. | `docs/design.md`; local API package |
| CLI-003 | retain | required | Nonblocking coalesced observations | Store revision and per-subscriber revision/topic/materialized-view wakes have bounded latest-value behavior. TUI selection uses an independent capacity-one control and a generation-scoped non-destructive Unix wake, so it never waits for a command, closes a healthy daemon connection, or loses a partial frame. | `docs/design.md`; store actor and materialized-view tests; local session-pump pressure tests; local API subscription tests; Unix control-wake generation/decoder tests; blocked-command TUI executor tests |
| CLI-004 | redesign | required | Node auto-start and readiness | Concurrent clients converge on one owner and receive phase/path/cause/action diagnostics on failure. | `docs/rust/cli.md`; `hq-node::LocalNodeClient`; concurrent readiness CLI test |
| CLI-005 | redesign | required | Lifecycle status, stop, and restart | Typed lifecycle control drains ownership; clients reconnect/resubscribe and detect build/version incompatibility. | `docs/rust/cli.md`; node/local API runner and real subscribed Unix restart/reconnect tests |
| CLI-006 | retain | required | Agent ask/send/wait/poll workflows | Preserve blocking request/response, asynchronous send, later wait, ready-mail polling, stable IDs, and no routine timeout. | README and `internal/agenthelp`; CLI/application packages |
| CLI-007 | redesign | required | Current-session mailbox discovery | Detect Codex/Claude Code/Pi context or explicit custom identity without ambiguous multi-provider routing. | `docs/rust/cli.md`; CLI provider-environment unit/redaction and foreground E2E tests |
| CLI-008 | redesign | required | Known-message inspection | Typed direct-ID inspection is non-consuming and does not become an authority bypass. | README and agent help; application/CLI packages |
| CLI-009 | redesign | required | Human list/answer/cancel/restore workflows | Expose typed filtering, reply, archive/cancel, restore, delivery, and causal state through application services. | README and CLI dispatcher; node-resolved mailbox-command service, stable replay/stale-target tests, and installed CLI restart test |
| CLI-010 | redesign | required | Administrative workflows | Identity, human, peer, capability, relay, sync, status, repair, node, agent/session, and configuration operations have client commands. Identity/configuration are implemented as exclusive offline commands; remaining administration uses the local API. | `docs/rust/cli.md`; administration parser/help/machine-output tests; named-agent, relay, repair, runtime, and project foreground E2E tests |
| CLI-011 | redesign | required | Project workflows | Expose every required project/resource/worktree/remote-result operation from ADR 0003. | `docs/projects.md`; complete project/resource parser and machine-output tests; fake and real-Git worktree response-loss/restart tests; foreground lifecycle, mutation, worktree, and restart E2E; remote-home signed-warning fixture |
| CLI-012 | redesign | required | Machine-readable automation | Supported commands emit stable typed Rust-era data and errors without preserving Go JSON. | `docs/rust/cli.md`; CLI machine-output, project catalog/operation determinism, and redaction fixtures |
| CLI-013 | redesign | required | Pure Ratatui state/effect architecture | `UiModel + UiEvent` produces effects; renderer performs no I/O or mutation; a materialized Inbox event installs list and selected detail atomically, with bounded revision-tagged first-page retention and identity-scoped stale suppression. | `docs/rust/tui.md`; `hq-tui` materialized navigation/stale/cache/reconnect and no-loading render tests; `hq-node` retained-startup, split command/observation ownership, blocked-command selection, shutdown, mapping, and effect-identity tests; architecture verifier |
| CLI-014 | retain | required | TUI semantic mailbox workflows | Open/sent/archived filters, mixed histories, answer/archive/restore, new direct/self messages, details, activity, and status are available. | `docs/rust/tui.md`; authoritative filtering and mixed-page tests; pure reply/direct/self/archive/restore model and executor tests; activity-target exclusion; installed TUI/CLI self-note parity |
| CLI-015 | retain | required | TUI durable editing state | Drafts, target reselection, focus, modal state, and logical scroll anchors survive reload/reconnect/resize as appropriate. | `docs/rust/tui.md`; store restart/conflict/stale-target contracts; pure autosave/save-before-close/save-before-submit/conflict/reload/reconnect/resize tests; responsive modal render tests |
| CLI-016 | redesign | required | TUI agent/session management | Search agents, inspect durable selection/history, start/exact-resume/rename/stop, and conservatively confirm switches without losing mailbox state. | `docs/rust/tui.md`; pure stable-search/detail/create/rename/retirement and managed-lifecycle model tests; responsive detail/switch rendering; exact executor/client-command mapping; installed TUI explicit-provider rejection and CLI stop/stale-resume restart coverage |
| CLI-017 | redesign | required | TUI project-first flows | Create/select/reopen/assign/activate/handoff/close/archive/resource/worktree flows expose conflicts and saga state. | `docs/projects.md`; TUI/application packages |
| CLI-018 | retain | required | Terminal restoration | RAII restores terminal mode on normal exit, error, panic, and node/client failure. | Rewrite acceptance matrix; TUI package |
| CLI-019 | redesign | required | Embedded/installed agent guidance | Concise help explains messaging, retries, sync, causal incompleteness, and human-owned administration. | `hq agents`; generated help/guidance snapshots in `hq-node::cli` |
| CLI-020 | redesign | required | One installed `hq` executable | Client, TUI, lifecycle, and explicit foreground node roles share distribution while keeping internal boundaries. | ADR 0001; `hq-node/src/bin/hq.rs`; CLI parser/help snapshots |
| CLI-021 | drop | excluded | Direct client SQLite, signer, relay, or provider ownership | Every supported client operation crosses application/local API boundaries. | Rewrite architecture; dependency tests |
| CLI-022 | drop | excluded | Go TUI key and layout percentages as contract | Rust supplies discoverable usable controls and representative render tests on its own layout. | ADR 0003; TUI package |
| CLI-023 | redesign | required | Repository-aware mailbox discovery | List candidate mailboxes using typed repository/worktree/branch/remote context and durable agent labels without claiming or merging them. | README and CLI dispatcher; application/CLI packages |
| CLI-024 | redesign | required | Help and version inspection | Ship product, workflow, protocol/build, and compatibility guidance without preserving Go help prose or version output. | `docs/rust/cli.md`; root/daemon help snapshots and human/JSON version tests |
| CLI-025 | drop | excluded | Go TUI GitHub pull-request enrichment | Remote PR lookup is not part of the retained HQ messaging/project boundary; repository/source context remains. | README TUI details; TUI package |
| CLI-026 | redesign | deferred | Mobile or remote rich client | Local API v1 serves same-user local clients; a network-facing/mobile client needs a separate authentication and transport specification. | `docs/projects.md`; future client package |

## Operations, security, recovery, and release scope

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| OPS-001 | redesign | required | Linux and macOS support | Unix local IPC, secure paths, ownership, signals, terminal behavior, and service guidance pass on four release targets. | ADR 0001; node/packaging/CI packages |
| OPS-002 | redesign | deferred | Windows product support | Add named pipes, ownership, lifecycle, secure paths, CI, and equivalent acceptance evidence before advertising it. | ADR 0001; future platform package |
| OPS-003 | retain | required | Bounded producers and storage | Every potentially unbounded queue/input/quarantine/activity/log path has backpressure, coalescing, rejection, or retention limits. | Rewrite invariant; all adapter packages |
| OPS-004 | retain | required | Secret and environment redaction | Tests prove exclusion from facts, projections, receipts, protocol diagnostics, logs, status, and RPC; provider stderr is a disclosed local boundary. | Rewrite security matrix; qualification package |
| OPS-005 | redesign | required | Structured actionable diagnostics | Stable categories and fields identify phase, object, path, cause, and safe next action without behavioral prose parsing. | Rust port and projects docs; domain/application packages |
| OPS-006 | retain | required | Ordered graceful node shutdown | Stop intake, preserve inbound/outbound work, drain providers, reconcile sagas, close store, wait/escalate tasks, then release ownership. | Rewrite lifecycle; node qualification |
| OPS-007 | retain | required | Crash recovery evidence | Failpoints at every durable/external boundary recover old valid, new valid, or explicit reconcilable uncertainty, never a hybrid. | Rewrite acceptance matrix; store/harness/relay/project packages |
| OPS-008 | redesign | required | Performance budgets | Record quantitative rebuild, late-parent, paging, readiness, redraw, queue, memory, and shutdown gates before qualification. | Rewrite design; performance/qualification packages |
| OPS-009 | redesign | required | Controlled dogfood and recovery drills | Exercise new identities/state, backup, restart, catch-up, relay/provider failure, repair, and node replacement before cutover. | `docs/lan.md` and rewrite plan; qualification package |
| OPS-010 | redesign | required | Read-only Go archival and cutover procedure | Rehearse archive of Go binary/key/database/logs and Rust sole-owner launch without performing production cutover automatically. | Rewrite design; cutover package |
| OPS-011 | drop | excluded | Automatic production cutover or soak declaration | Disabling Go, activating production identity, and declaring soak success require separate operator authority. | Rewrite autonomous boundary; cutover package |
| OPS-012 | drop | excluded | Proof of external resource/process cessation | HQ claims are advisory and runtime observations cannot prove arbitrary external actors stopped. | Rewrite non-goal and projects docs; security documentation |
| OPS-013 | redesign | required | Architecture enforcement | CI forbids runtime/SQLite/Nostr/Ratatui/filesystem/process/provider dependencies in the pure core and provider vocabulary in neutral crates. | Rewrite acceptance matrix; workspace package |
| OPS-014 | redesign | required | Release evidence map | Every acceptance-matrix row and definition-of-done clause links to direct current test, fixture, benchmark, drill, or artifact evidence. | Rewrite design; qualification package |
| OPS-015 | redesign | required | Test-only conformance trace v1 | Deterministic semantic operations and normalized observations support reviewed fixtures without constraining production protocols or making Go equality authoritative. | Rewrite protocol ownership; testkit/qualification packages |
| OPS-016 | retain | required | Cooperative same-user local trust and plaintext-domain disclosure | Secure permissions and central ownership do not claim at-rest secrecy from the same OS user; local content storage and provider-stderr boundaries are documented. | README and `docs/lan.md`; security documentation/tests |
| OPS-017 | retain | required | Add-only canonical retention | Verified semantic facts remain immutable canonical knowledge without automatic first-release pruning; disposable projections may have deterministic retention. | `docs/events.md`; store/repair packages |

## Inherited Go findings as Rust regression requirements

| ID | Classification | Release | Capability or behavior | Rust contract | Evidence and downstream owner |
| --- | --- | --- | --- | --- | --- |
| REG-001 | retain | required | `REG-AUTHORITY-MAXIMAL-REGRANT` | A regranted device is authoritative only through a causal-maximal acceptance descending from the revoke; lexicographic historical selection is forbidden. | Former Go plan finding; authority race matrix |
| REG-002 | retain | required | `REG-CONVERSATION-COMPARATOR` | The reducer owns one comparator for all messages/activity; store/UI indexes consume it and equal-time cases remain deterministic. | Former Go plan finding; reducer/query tests |
| REG-003 | retain | required | `REG-INDEXED-PAGINATION` | Stable cursor/index pagination concatenates to canonical order and later pages do not load or sort complete history. | Former Go plan finding; store performance tests |
| REG-004 | retain | required | `REG-NONDISRUPTIVE-RELAY-WAKE` | Publish/config wakes coalesce and prompt work without restarting a healthy retained subscription. | Former Go plan finding; relay session tests |

## Go command and TUI workflow cross-check

This index maps every top-level Go command family and every documented TUI workflow to a behavior
row. It proves workflow coverage; it does not reserve the command names.

| Go surface | Product outcome | Classification and row |
| --- | --- | --- |
| bare `hq` in a terminal; `hq tui` | Launch the semantic Ratatui mailbox client. | redesign: `CLI-013` through `CLI-018` |
| bare `hq` without a terminal | Present the human inbox through a noninteractive client. | redesign: `CLI-009`, `CLI-012` |
| `hq agents [topic]` | Give agents concise messaging, sync, and delivery guidance. | redesign: `CLI-019` |
| `hq ask` | Send a question and wait without a routine timeout. | redesign: `MSG-001`, `CLI-006` |
| `hq send` | Persist an asynchronous message and return stable identity. | redesign: `MSG-002`, `CLI-006` |
| `hq wait` | Wait for the first eligible answer to a question sent by this mailbox. | redesign: `MSG-003`, `CLI-006` |
| `hq poll` | Consume ready replies and unsolicited mailbox messages with duplicate-safe identity. | redesign: `MSG-002`, `MSG-015`, `CLI-006` |
| `hq get` | Inspect a known message without consuming it. | redesign: `CLI-008` |
| `hq list` | Filter human-visible open, archived, sent, sender, recipient, and repository views. | redesign: `CLI-009`, `CLI-012` |
| `hq answer`; `hq cancel` | Reply and archive/cancel through causal facts. | redesign: `MSG-001`, `MSG-004`, `MSG-005`, `CLI-009` |
| `hq mailboxes` | Discover repository-related mailbox candidates and agent labels. | redesign: `CLI-023` |
| `hq agent create/list/retire` | Manage permanent named-agent identity and retirement, including safe quiescence. | redesign: `RUN-001`, `RUN-002`, `PRJ-015` |
| `hq harness start|resume|stop` | Start, exact-resume, or stop a managed provider session through the local API; report rejection and uncertainty explicitly. | redesign: `RUN-003`, `RUN-006` through `RUN-009`, `RUN-017`; CLI parser/request tests, reconnecting-client exact-frame tests, and `unix_node_cli` restart/stale-resume coverage |
| `hq project create/list/show/send/activate/handoff/close/reopen/archive/unarchive/check` | Manage the full project lifecycle, assignment, health, messaging, and workflow outcomes. | redesign: `PRJ-001` through `PRJ-020`, `CLI-011` |
| `hq project resource add/remove/primary/replace` | Manage desired paths, primary selection, active claims, conflict, and health. | redesign: `PRJ-004`, `PRJ-010` through `PRJ-013` |
| `hq project worktree` | Provision a recoverable Git worktree and project with one stable operation. | redesign: `PRJ-019`, `CLI-011` |
| `hq identity init/show/export/import` | Initialize, inspect, back up, and restore Rust identity safely. | redesign: `IDN-001` through `IDN-007` |
| `hq identity reset --yes` | Recursively remove identity and state as a routine product command. | drop: `IDN-004` |
| `hq human show/invite/join/devices/revoke` | Inspect, pair, accept, list, and revoke human-account devices. | redesign: `IDN-011` through `IDN-015`, `CLI-010` |
| `hq peer add/list/distrust` | Manage directional peer routing/trust metadata. | redesign: `IDN-009`, `CLI-010` |
| `hq mailbox share/revoke` | Grant and revoke peer mailbox capability. | redesign: `IDN-010`, `CLI-010` |
| `hq relay add/list/remove` | Configure bounded read/write/auth relay policy. | redesign: `NET-010`, `CLI-010` |
| `hq status`; `hq sync` | Inspect processing/delivery health and request prompt node synchronization. | redesign: `NET-010`, `NET-011`, `CLI-010` |
| `hq daemon run/status/stop/restart` | Run and control the sole owning node with graceful lifecycle and reconnect. | redesign: `CLI-004`, `CLI-005`, `CLI-020` |
| `hq config get/set` | Manage typed installation-local provider/relay preferences. | redesign: `IDN-007`, `CLI-010` |
| `hq help`; `hq version` | Inspect supported workflows and build/protocol compatibility. | redesign: `CLI-024` |
| global database/path selector | Choose an explicit Rust installation root without opening Go state. | redesign: `CMP-007`, `IDN-007` |
| global immediate-sync suppression | Rust mutations send no implicit prompt wake; `relay sync` requests one explicitly and never claims node-wide offline operation. | redesign: `NET-011` |

The 2026-08-28 non-TUI parity audit matched every retained row above against the closed parser,
root/topic help snapshots, stable human/JSON renderers, and foreground Unix tests. It closed the
last two gaps: recoverable `project worktree` provisioning and bare noninteractive `hq` inbox
listing. Terminal-only rows are covered by the pure Ratatui model and render suites plus installed
pseudoterminal workflow tests.

| Documented TUI workflow | Product outcome | Classification and row |
| --- | --- | --- |
| Inbox/sent/archived filtering and conversation selection | Preserve semantic human mailbox views and open-work grouping. | retain/redesign: `CLI-014`, `MSG-005` |
| Mixed message/activity history and canonical scrolling | Consume reducer order, expose activity separately, and preserve logical anchors. | retain: `MSG-008` through `MSG-011`, `CLI-015` |
| Reply, archive, restore/undo, direct new message, and self-note | Support human action workflows without body parsing or selected-row leakage. | redesign: `CLI-009`, `CLI-014` |
| Durable reply/new-message drafts and stale-target recovery | Save installation-local edits and submit/consume atomically. | redesign: `MSG-016`, `CLI-015` |
| Technical detail, delivery, relay, account, and causal disclosure | Show typed actionable state while keeping diagnostic sections non-authoritative. | redesign: `MSG-007`, `MSG-013`, `MSG-014`, `NET-010` |
| Named-agent/session search, start, resume, rename, switch, and stop | Manage durable selection separately from runtime presence. | redesign: `RUN-002`, `RUN-017`, `CLI-016` |
| Project-first create/compose/resource/activation/handoff/lifecycle modals | Expose all project invariants, conflicts, confirmations, and saga outcomes. | redesign: `PRJ-001` through `PRJ-020`, `CLI-017` |
| Resize, focus, keyboard/mouse interaction, Markdown, and responsive layout | Remain usable and deterministic without preserving Go keys, percentages, colors, or cells. | redesign/drop boundary: `CLI-013`, `CLI-018`, `CLI-022` |
| Reconnect/invalidation reload with state preservation | Recover authoritative snapshots without losing applicable local UI state. | retain: `CLI-001` through `CLI-003`, `CLI-015` |
| Automatic GitHub pull-request lookup | Omit remote UI enrichment while retaining repository context. | drop: `CLI-025` |

## Accepted boundary decisions

- `docs/adr/0001-rust-platform-and-packaging.md`
- `docs/adr/0002-rust-identity-backup-boundary.md`
- `docs/adr/0003-rust-client-and-provider-workflows.md`

Protocol field layouts, Nostr application kind, local framing, exact state paths, Codex baseline,
performance numbers, and soak thresholds remain owned by their named downstream specification
packages. That is deliberate delegation of detail, not an implicit Go default. The rows above fix
the outcome and release boundary those specifications must satisfy.
