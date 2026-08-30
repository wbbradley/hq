# Completed

## 2026-08-29 — Clap-owned CLI grammar and generated help

Replaced the installed client's handwritten parser and duplicated help matrix with one private,
state-free Clap grammar. Clap 4.6.6 is workspace-pinned with only the std, help, usage, and
error-context features. The grammar owns the complete command tree, global option placement,
cardinality, defaults, conflicts, paired provider/session options, set/clear and archive/all
choices, assignment session modes, repeated relay hints, and explicit destructive confirmation.
The mapping boundary preserves the existing closed command types and keeps domain validation for
IDs, names, durations, content, relays, and paths outside Clap.

Root, command, and deeply nested help are generated from the same grammar and retain operational
and safety guidance. Clap never prints, exits, or exposes rejected values: human and JSON failures
continue through HQ's stable redacted usage diagnostic. Raw non-UTF operating-system paths remain
accepted where commands take PathBuf values, textual values remain UTF-8, global options now work
before or after subcommands, and both --output json and --output=json select typed errors. The
executable's separate bare-terminal TUI/list default remains unchanged.

Parser and help tests now cover generated nesting, relationships, global placement, raw paths, and
redaction. The Rust CLI guide documents the authoritative generated grammar. An older PTY assertion
was made robust against Ratatui differential cursor output while preserving its three-intent copy
contract. Formatting, architecture verification, locked workspace check/test/build, strict
workspace Clippy, and the dependency-policy audit pass; all 25 installed CLI scenarios, 12 PTY
scenarios, 77 node unit tests, 68 TUI model tests, and 29 render contracts pass.

### Original plan entry

### Replace the handwritten CLI grammar with Clap

Inventory the current accepted and rejected invocation matrix, decide the intended grammar, then replace the handwritten command grammar with a minimally featured, workspace-pinned Clap dependency. Backwards compatibility is explicitly a non-goal because HQ has not shipped: preserve the valuable architectural and safety boundaries, but freely simplify command spelling, option relationships, help, diagnostics, and structured output where the new grammar exposes a better design. Update tests, documentation, and internal consumers atomically for every intentional change.

- Add Clap to the workspace and `hq-node` dependency manifests, with only the features the grammar adapter needs, and update the lockfile. The dependency must pass the repository's dependency-policy audit and remain isolated to the CLI adapter.
- Introduce a private Clap grammar representation, preferably in a focused module such as `crates/hq-node/src/cli/grammar.rs`. Clap should own the root and nested command tree, positional and option cardinality, defaults, aliases, conflicts, requirements, and generated help. Keep `parse_cli` as the state-free `OsString` entry point and map the private representation into the existing `CliInvocation`/`CliCommand` tree so execution code does not become coupled to Clap.
- Keep HQ domain validation custom: canonical 32-byte IDs, agent and provider names, durations, bounded content and labels, relay URLs and hints, and absolute/canonical path policy should continue through existing constructors or narrowly scoped value parsers. Preserve raw non-UTF path support everywhere the current parser accepts `PathBuf`, while continuing to reject non-UTF values for textual fields.
- Review and deliberately model option relationships such as provider/session pairs, `--archived` versus `--all`, rename clear versus display name, project activation and handoff session choices, destructive confirmation, repeated relay hints, and the TUI's output-mode restriction. Existing relationships are design input, not compatibility requirements; prefer the clearest consistent Clap-native grammar and document intentional changes.
- Do not allow Clap to print, terminate the process, or expose rejected argument values. Preserve diagnostic redaction as a security property and produce a consistent nonzero usage-error class. Help, version, human diagnostics, and the `hq-cli-output-v1` JSON shape may be revised rather than emulated if doing so yields a cleaner contract; keep HQ's authoritative build/protocol version data and update all affected tests, docs, and consumers together.
- Replace the manually duplicated grammar in `help_text` with Clap-generated root and nested help, while carrying forward all operational and safety guidance as command metadata such as `long_about` or `after_help`. Snapshot command ordering, usage, required arguments, conflicts, aliases, and semantic warnings so generated help remains intentional and reviewable.
- Keep the executable's pre-parse TTY default outside Clap: bare `hq`, including invocations containing only global options, must continue to choose `tui` when stdin and stdout are terminals and `list` otherwise. Preserve daemon descriptor isolation, TUI dispatch, password-stdin selection, and the no-prompt/state-free parsing boundary in `crates/hq-node/src/bin/hq.rs`.
- Remove the handwritten `parse_*` grammar functions and obsolete help matrix only after tests cover the intended behavior for every command family; duplicate, unknown, missing, and conflicting options; invalid UTF-8 text versus raw paths; value bounds and invalid absolute paths; help and version; global-option placement; diagnostic redaction; JSON errors; and bare TTY/non-TTY behavior. Use the existing unit tests, installed-CLI tests, and CLI behavior ledger as an inventory of cases to reconsider, not as a compatibility baseline; delete or rewrite assertions that only preserve accidental parser behavior.
- Update `docs/rust/cli.md` for the resulting command semantics and to explain that Clap owns command grammar and generated help. Finish with formatting, architecture verification, locked workspace check/test/build, strict Clippy, and the dependency-policy audit.

Shell completions and broader execution-layer decomposition are out of scope.



## 2026-08-29 — First-run guidance and fresh-user acceptance

Turned bare interactive `hq` into one ordered first-run journey. Missing device identity now fails
before terminal activation with a plain-language reason, one exact setup action, and the command
that resumes onboarding. Identity and successful human-account output leave the next action on
screen. An identity-only TUI explains what the account is for, gives account creation as the one
primary action, keeps invitation-based joining in contextual help, and offers an in-place F5 reload
so the user does not have to remember or reconstruct a previous screen.

The ordinary empty Inbox now shows one current onboarding step at a time: project and folder or
resource ownership, agent creation, agent-service readiness, and the first project instruction.
Completed prerequisites stay visible. Provider namespaces remain impossible to type: one available
service is automatic, several remain typed choices, and none becomes its own explained setup state.
The project path continues to recommend an existing folder while keeping Git worktree creation in
the advanced option.

F1 now opens help from every ordinary screen and dialog without consuming text input, closing the
interaction, or losing its fields; F5 refreshes authoritative state with the same retention. Dialog
help explains the user's current decision, while technical evidence remains separately disclosed.
The documented first-run walkthrough includes terminal captures and an acceptance ledger spanning
fresh account setup, folder-backed projects, agents, provider choice, first input and exact
conversation return, direct-message discovery, help, restart, and reconnect. The installed
pseudoterminal harness now serializes complete fresh-state setup and synchronizes on visible state,
making all twelve scenarios deterministic under the full workspace suite.

Formatting, architecture verification, qualification-evidence validation, strict workspace
Clippy, locked workspace check/test/build, 68 pure-model tests, 29 render contracts, and all 12
installed pseudoterminal scenarios pass.

### Original plan entry

### Add first-run guidance and fresh-user acceptance coverage

Make bare `hq` useful without prior knowledge of commands or domain terminology.

- Detect missing identity, missing human account, missing providers, no projects, and no agents as
  distinct setup states. Present an ordered setup path in the TUI or, where the node cannot yet run,
  in a focused pre-TUI screen with one exact action and an explanation of its result. Do not present
  authority jargon as onboarding copy.
- After account setup, lead to the ordinary empty TUI and the `New...` workflow rather than a dead
  end. Never require users to remember a command printed on a previous screen.
- Add scenario tests beginning with a fresh state root and covering account setup, folder-backed
  project creation, agent creation, provider selection, first project instruction, return to the
  resulting conversation, direct-message discovery, contextual help, restart, and reconnect.
- Conduct a copy and interaction audit from the perspective of a user seeing every screen for the
  first time. Remove unexplained nouns, raw state codes, silent keys, success acknowledgements that
  require dismissal, and dead ends. Record the final walkthrough and screenshots in
  `docs/rust/tui.md`.

## 2026-08-29 — Guided New workflow

Added a global `n` launcher that starts from user intent: work with an agent on a project, send a
direct message, or write a personal note. The project path selects or creates a project, prefers
unassigned agents and can create one in place, asks for a typed provider choice only when multiple
services are available, collects the instruction, and presents a compact plain-language review.
The existing `d` direct-message and `N` personal-note shortcuts remain available.

The workflow exact-resumes compatible project conversations and skips unnecessary setup for
already-runnable assignments. Otherwise it drives the existing retry-safe activation or handoff,
waits for the authoritative runnable snapshot, and emits the retained instruction exactly once.
Provider catalog refreshes preserve the draft. Assignment and resource conflicts name the competing
project without mutating state, while rejected or reconcilable outcomes preserve the complete review
and draft for recovery and cannot later trigger a silent send.

Successful sends select and open the exact conversation in Sent with a persistent context banner
naming its project, agent, and provider. Direct sessions, messages, notes, and project/resource
administration remain independent capabilities. Pure-model and rendering coverage exercises every
wizard screen and the recovery paths; ten installed pseudoterminal workflows cover the user-facing
launcher. Formatting, architecture verification, qualification validation, strict Clippy, locked
workspace check/test/build, and all focused TUI suites pass.

### Original plan entry

### Add an extensible guided `New...` workflow

Provide a clear path from intent to conversation while keeping project work, direct messages, and
personal notes distinct.

- Make `n new` open a small launcher with `Work with an agent on a project`, `Send a direct message`,
  and `Write a personal note`. Retain expert shortcuts where useful. Structure direct-message target
  selection so future human peers can appear alongside other typed recipients without changing the
  project workflow or inventing provider sessions for humans.
- For project work, guide the user through selecting or creating a project, selecting or creating an
  unassigned agent, and choosing a provider only when necessary. Show a compact review in user
  terms before any materially different assignment or handoff.
- Orchestrate the existing retry-safe project activation/session operations behind the workflow.
  Resume a compatible existing project conversation when selected; otherwise create the required
  scoped session and assignment, then open the ordinary project message composer.
- Explain exceptional choices in place: an agent assigned to another project requires an explicit
  handoff path; resource or assignment conflicts identify the competing project; rejected or
  uncertain setup retains the user's selections for recovery.
- On success, open a new message addressed to the project and its assigned agent. Do not expose the
  user to an empty Inbox, operation outcome dialog, or a separate unexplained managed-session step.
- Preserve direct agent sessions, direct messaging, notes, and project/resource administration as
  independent capabilities. The guided project path is a convenience over the domain model, not a
  replacement for HQ's broader collaboration model.

## 2026-08-29 — Contextual TUI completion

Replaced routine project and managed-session outcome dialogs with one typed completion policy.
Ordinary successes now close their form, refresh from the authoritative snapshot, and show a
four-second green footer confirmation that any user input may dismiss immediately. Confirmation
copy is bounded and user-facing; operation identifiers and recovery evidence remain available only
where they help diagnose an exceptional result.

Completion navigation is retained independently of the transient notice. New projects are selected,
ordinary project changes return to project details, activation and handoff continue into the first
instruction composer, and advanced managed-session actions return to the refreshed agent with the
exact provider session visible. A pre-command snapshot cannot strand navigation: HQ requests one
bounded follow-up snapshot when the target is initially absent. Snapshot failure and reconnect
retain the continuation, while a target still absent from the follow-up becomes an explicit stale
completion failure instead of silently selecting the wrong object.

Running, rejected, uncertain, reconcilable, conflict, preview, and resource-check outcomes remain
modal because they carry a decision or material evidence. Pure-model tests cover every project and
managed-session outcome class, timer and input dismissal, reconnect, stale completions, and the
pre-command snapshot race. Render and installed pseudoterminal tests verify that routine completion
requires no acknowledgement dialog. Formatting, architecture verification, qualification-evidence
validation, strict workspace Clippy, locked workspace check/test/build, all HQ TUI targets, and all
nine installed TUI pseudoterminal workflows pass.

### Original plan entry

### Replace routine outcome dialogs with contextual completion

Create one typed presentation policy for command completion instead of opening an outcome modal for
every project and managed-session response.

- On ordinary success, close the form, request the authoritative snapshot, select the created or
  changed object, and show a bounded transient confirmation in the footer/status area.
- When success has an obvious continuation, navigate to it: project creation selects the new project;
  starting project work opens its conversation or first-message composer; manual advanced session
  administration returns to the agent with the selected session visible.
- Keep a modal only when the user must make another decision, when a preview must be committed, or
  when the result is rejected, uncertain, reconcilable, conflicted, or otherwise unsafe to dismiss.
  Preserve operation identity and exact recovery evidence in those exceptional states.
- Define and test completion behavior for every `UiProjectOutcome` and
  `UiManagedSessionOutcome`, including reconnect and stale-completion cases, so no success path
  strands the user or discards context.

## 2026-08-29 — Typed provider choices

Added a bounded, passive provider catalog from the harness registry through the application and
local-API boundaries into the TUI presentation snapshot. Catalog entries carry stable provider
identity, a user-facing name, current availability, and configured-default status without exposing
provider handles or coupling the TUI to Codex. A stale configured default remains visible as an
unavailable entry so configuration drift can be explained instead of silently hidden.

Removed free-text provider namespace editing from ordinary managed-agent and project activation or
handoff workflows. One available agent service is used automatically; several render a typed
chooser that prefers the available configured default and skips unavailable entries; none render
actionable setup guidance and cannot submit. Catalog refresh replaces a vanished choice with a
current valid one and reports the change, while exact saved-conversation resumes retain their
historical provider/session identity and technical views retain raw evidence.

Protocol, routing, registry, node mapper, pure model, responsive render, and installed terminal
tests cover empty, single, multiple, unavailable, defaulted, and stale catalogs. Documentation now
defines the catalog as node-local passive metadata and records the shared selection policy.
Formatting, architecture verification, qualification-evidence validation, strict workspace Clippy,
locked workspace check/test/build, all HQ TUI targets, and all nine installed TUI pseudoterminal
workflows pass.

### Original plan entry

### Expose providers as typed available choices

Remove every free-text provider namespace field from ordinary TUI workflows.

- Add a passive provider catalog to the node/TUI boundary, sourced from the providers actually
  registered and available to the running installation. Include stable identity, user-facing name,
  availability, and the configured default without coupling the pure TUI model to a concrete
  provider implementation.
- Render a choice control when several providers are available, select the configured default when
  possible, automatically use the only available provider, and show an explanatory empty state when
  none are available. Never ask the user to guess that `codex` is valid.
- Keep raw provider namespaces and exact session identities in technical details and advanced session
  administration. Design the catalog protocol so adding providers extends the list rather than the
  form grammar.
- Cover zero, one, several, unavailable, defaulted, and stale-provider cases with protocol,
  mapper, model, and render tests.

## 2026-08-29 — Project creation centered on resource ownership

Replaced the two competing project-creation entry points with one `Create project` chooser. Its
recommended path records an existing folder, while isolated Git worktree creation is progressively
disclosed as an optional advanced convenience and remains available through an expert shortcut.
The footer and empty state now describe the user's intent without positioning HQ as a worktree
manager.

The folder workflow starts with the resource path, derives the project name from the normalized
folder even when the form is submitted directly, labels the brief as optional, and explains both
the ownership claim and HQ's non-ownership of routine filesystem and Git maintenance. Before any
creation mutation, the TUI now performs a typed, read-only resource inspection against the complete
authoritative catalog. A clear preview requires an explicit commit; a conflict names the owning
project and its displayed path, emits no creation command, and returns to the retained form for
editing.

The existing project/resource domain model and worktree saga remain unchanged. Creation previews
reuse the same canonical path inspection and overlap policy as ordinary resource mutations, while
worktree failures retain their exact reconciliation and external-state evidence in technical
details. Model, mapper, responsive-render, and installed pseudoterminal coverage exercise the
chooser, folder defaults, conflict preflight, form recovery, direct worktree shortcut, and both
creation modes. Formatting, architecture verification, qualification-evidence validation, strict
workspace Clippy, locked workspace check/test/build, all HQ TUI targets, and all nine installed TUI
pseudoterminal workflows pass.

### Original plan entry

### Clarify project creation and resource ownership

Make project creation explain the durable HQ concept—resource ownership—without positioning HQ as
a Git worktree manager.

- Change the Projects footer to `c create`. Let `c` open one `Create project` chooser whose primary
  path is `Use an existing folder` and whose optional advanced path is `Create an isolated Git
  worktree`. An expert shortcut may open the worktree path directly, but it should not dominate the
  default footer or empty state.
- Rename `Create project from existing tree` to `Create project from folder` and explain that HQ will
  record the folder as a project resource; it will not take over ordinary filesystem or Git
  maintenance.
- Rename `Create recoverable Git worktree project` to `Create an isolated Git worktree`. Explain in
  user terms that this creates a branch and separate working directory while retaining external
  files if setup is interrupted. Keep reconciliation and retained-external-state evidence in the
  failure/details path.
- Default the project name from the selected folder where possible, label the brief as optional,
  preview the resource path and ownership implications, and report claim conflicts before commit in
  terms of the conflicting project and path.
- Preserve the authoritative project/resource model independently of how a directory or worktree was
  created so future agent-managed worktrees require no project-model redesign.

## 2026-08-29 — Familiar TUI forms

Introduced one pure, reusable form editor for project creation and input, resource paths,
activation and handoff, agent creation and conversation naming, searches, managed-session setup,
and mailbox composition. Text fields now share Unicode-safe Left, Right, Home, End, Backspace, and
Delete behavior, atomic bounded paste, visible insertion carets, and cursor state that survives
resize, reconnect, authoritative refresh, and in-flight command recovery. Tab and Shift-Tab move
through multi-field dialogs; Up and Down remain choice/list controls, and modal editing continues to
take precedence over global section navigation.

Forms identify required and optional fields, show focused examples or concise guidance, and render
known validation next to the relevant field before emitting an effect. Agent slug validation now
matches the installed CLI grammar. Project paths use one lexical boundary that expands only exact
`~` and `~/...` through the operating-system user's account record, rejects relative and shell-like
expressions, displays the normalized absolute path, and still delegates canonical filesystem and
resource-ownership checks to the existing domain workflow.

Pure editor/model tests cover Unicode insertion and deletion, atomic paste bounds, path expansion
and shell non-expansion, reverse field traversal, and caret preservation through authoritative
reload. Responsive render coverage checks project, agent, and mailbox form guidance and carets.
Crossterm and installed pseudoterminal tests exercise the expanded key vocabulary and Tab-based
project workflows. Formatting, architecture verification, qualification-evidence validation,
strict workspace Clippy, the locked all-target/all-feature workspace suite and build, and all nine
installed TUI pseudoterminal workflows pass.

### Original plan entry

### Make dialogs behave like familiar forms

Introduce reusable form-editing and rendering behavior rather than continuing per-dialog key and
cursor logic.

- Make `Tab` and `Shift-Tab` move forward and backward through fields in every multi-field form.
  Keep arrow keys for list/choice navigation and text-cursor movement according to ordinary terminal
  conventions; modal input must take precedence over global section navigation.
- Render both a clear focus treatment and a visible insertion caret. Support Unicode-safe left,
  right, Home, End, Backspace, and Delete behavior, bounded paste, and predictable submission and
  cancellation. Preserve in-flight and reconnect-safe inputs.
- Mark required and optional fields, show examples or concise field guidance, and attach validation
  messages to the relevant field before submission. Do not make users infer requirements from a
  rejected operation code in the global footer.
- Add one path-input boundary that expands `~` and `~/...` for the current user, produces the
  absolute path required by the domain, and shows the normalized path before mutation. Do not
  expand arbitrary shell syntax or weaken canonical resource-identity validation.
- Exercise the reusable form behavior with model tests and terminal render snapshots, then migrate
  project creation, agent creation/rename, resource paths, project input, activation/handoff, and
  mailbox composition without duplicating editing policy.

## 2026-08-29 — Final completion-evidence hardening

Expanded the acceptance inventory from representative area-level examples to 70 direct current
proofs. The domain row names all nine algebra laws individually; the remaining rows directly bind
incremental/batch equality, stable cursor concatenation, concurrent readiness, nondisruptive relay
wakes, harness backpressure and worker release, expected-head and resource conflicts, retained pure
and installed TUI workflows, every quantitative budget, and each recovery drill. The validator
continues to reject missing paths, renamed selectors, duplicate proofs, and unknown proof kinds.

Strengthened the definition-of-done contract without duplicating its semantic clauses. Durable and
external recovery now has separate evidence for identity/database recovery, relay/provider failure,
project-saga response loss, and archived-Go rollback. The protocol verifier checks all four ADRs,
canonical/control/local/envelope/relay/pairing specifications, harness and Codex contracts,
conformance trace, vectors, links, and three executable consistency suites. The new Go-independence
gate proves Cargo metadata, production Rust sources, release packaging, the native release workflow,
service definitions, and Rust state paths do not invoke or include Go; tamper fixtures prove it
rejects a Go command and a Go release payload.

Applied the pre-release rules directly: the one unshipped clean-sheet SQLite schema is v1, with no
migration or compatibility path, and stale pre-implementation wording is gone. The shared
`HarnessSessionOperationState` remains the single stored/runtime state type. Existing contract tests
prove passive application, harness, relay, project, local-API, and TUI records expose fields
directly while invariant, capability, and secret-bearing types remain opaque. Replaced yanked
`chacha20` 0.10.1 with the compatible RustCrypto 0.10.2 release; the dependency audit reports no
yanked package.

The clean worktree passed formatting, architecture and Go-independence gates, behavior-ledger and
causal-spec validation, every required protocol-spec consistency suite, dependency policy, strict
workspace Clippy, locked check/build, the complete all-target/all-feature test suite, all local
qualification budgets, and every release/recovery validator fixture. No exact workspace-built HQ
daemon or executable-name `hq` process remained after the suite.

Actions run 33264900059 passed exact revision
`7317efae3aea99150c5d4d5eb3c729517fd11bb1` on Linux x86-64, Linux ARM64, Intel macOS, and Apple
Silicon, then passed controlled relay/provider failure, synthetic archived-Go rollback, and the
aggregate verifier. A fresh independent download passed all five validators and regenerated the
release, recovery, and cutover manifests byte-for-byte. The release, recovery, controlled-failure,
rollback, and cutover SHA-256 digests are respectively
`b71510aaa50ea743f924500b8e6c3026e4560eddd499a5edf41cd061dbe22d92`,
`4244542c918dec9490c216ed3d57334dea32a6cf61e241d702eec9fc5fc0c293`,
`a21cb98a5bf826c4661a50f5dad4e99953b6d23e94fd1b4c561e014ff78776c5`,
`36c9a975a14d12087326fac5ba4840032ccf14aba51e71e1472637ee8234b08a`, and
`d40e56906b5a35d88b0e4b1398c4f9701c9d0b6c828a7d1167d1318d700c13a0`.

The final governing-design audit found all 147 required ledger rows implemented, all 11 deferred
rows still explicitly outside first-release scope, and all 33 exclusions preserved. Every one of
the 11 acceptance rows and nine definition-of-done clauses has direct current evidence. All seven
pre-coding decisions are resolved in ADRs or named specifications. The release remains a v0.1.0
candidate only: no tag, publication, production identity access, live service activation, soak,
or cutover occurred.

### Original plan entry

- **[verification/high] Close completion-evidence coverage gaps** — Expand the acceptance inventory
  so every required subclaim in each matrix row names direct current executable evidence, bind the
  definition-of-done recovery clause to identity/database, relay/provider, project-saga, and
  archived-Go rollback evidence, and add an explicit verifier proving normal Rust build/runtime
  inputs have no Go code, state, protocol, service, or toolchain dependency. Remove stale
  pre-implementation markers from normative Rust documents, and normalize the unshipped
  clean-sheet storage schema to v1 without a migration or compatibility path. Replace any yanked
  cryptographic dependency discovered by the final dependency audit. Complete this work only after
  the exact revision passes local gates and the native release workflow, independently downloaded
  artifacts reproduce the release, recovery, and cutover manifests, and a final requirement audit
  finds no weak or missing proof.

## 2026-08-29 — Rust cutover package and evidence audit

Replaced the supported install and operator surface with the first Rust v0.1.0 candidate: four
native target mappings, checksum and embedded-revision verification, exact Rust state paths, the
pinned Codex 0.150.1 provider, and one-executable service management. The systemd definition now
uses an absolute executable marker and contains no Go path; both systemd and launchd guidance keeps
provider lookup narrow. The changelog states plainly that HQ has never shipped, so there is no
storage upgrade, migration, legacy compatibility, or standing-installation obligation.

Added an authorization-separated operator checklist. Soak permits only a new identity and state on
controlled relays; production cutover remains a later independent decision. The checklist covers
process/state-root inventory, read-only archival, exact service selection, recovery limits,
rollback triggers, and the invariant that unrelated HQ daemons are never killed by name.

Added a Linux packaged-binary rollback drill around an inaccessible synthetic Go binary, key,
database, and log. It starts only an explicit ephemeral Rust state, proves readiness and clean
shutdown, atomically changes an offline operator selector, compares archive metadata, and compares
the pre/post target-directory HQ process inventory. It never executes the Go binary, opens its
state after archival, touches production identity, or signals an unrelated process. Positive and
tamper fixtures enforce every evidence field.

The cutover contract covers all eleven acceptance-matrix areas and nine definition-of-done clauses.
Its aggregate binds the contract, four-host release manifest, four-host recovery manifest,
controlled relay/provider failure, and synthetic rollback evidence by SHA-256 while recording that
soak and cutover authorization remain unperformed.

Actions run 33257580370 passed every native package/recovery lane, the 103-second controlled failure
job, the 15-second rollback job, and aggregate validation for exact revision
`f408702866faeeb2530ecedff4a25f9786bea8be`. The combined artifact was downloaded independently;
all five validators passed and the release, recovery, and cutover manifests regenerated
byte-for-byte. The cutover bundle SHA-256 is
`b61b0997c87c46fd9a9c155f26e690e2c0925fd41bb1db3d5bc27338358a6fdf`.

Formatting, action/workflow syntax, ShellCheck, strict workspace Clippy, the locked complete
all-target/all-feature test suite, qualification inventory, and validator tamper tests pass. The
final acceptance and definition-of-done audit found no gap. No accessor facade, duplicate
stored/runtime state, storage/protocol bump, dependency, or lockfile changed; no tag, publication,
live service change, production identity access, soak, or cutover occurred.

### Original plan entry

- **[release/high] Complete the Rust cutover package and evidence audit** — Update supported-install
  and operator documentation, service-manager guidance, recovery boundaries, and the release
  changelog/version candidate. Rehearse rollback to an untouched archived synthetic Go
  installation without starting it or opening its state, produce the cutover checklist and
  evidence bundle, and audit every acceptance-matrix row and definition-of-done clause. Complete
  this work when an operator can separately authorize soak and cutover with known rollback steps;
  do not tag, publish, replace, disable, or activate any live installation.

## 2026-08-29 — Controlled relay and provider failure rehearsal

Added an isolated Linux x86-64 release-candidate drill that consumes the already packaged binary,
creates a new identity and state root, and owns a loopback-only pinned rnostr v0.4.9 container and
fresh relay data. The authenticated interoperability contract publishes a signed encrypted
kind-1059 wrapper, verifies retained recipient-filtered catch-up by the signed wrapper identity,
decrypts byte-identical embedded canonical data, and repeats after reconnect. This correctly
allows a Nostr relay to reserialize the outer JSON without weakening wrapper ID, signature,
recipient, or canonical-byte verification.

The drill stops the exact controlled relay, proves its endpoint is unreachable while the packaged
HQ daemon stays ready and accepts a synchronization wake, restarts the same relay data, repeats
catch-up, and verifies retained policy. Deterministic provider seams then prove redacted transport
crash containment, exact worker-lease release, response-loss and partial-persistence reconciliation,
forced teardown ownership release, ordered node drain, clean shutdown, and offline state-root
reacquisition. Exact traps remove only the rehearsal's temporary state and container; no standing
HQ state, identity, credential, relay, or unrelated process is inspected or changed.

Actions run 33256363580 passed all four native package and recovery lanes, the 94-second controlled
failure job, and aggregate validation for exact revision
`140d7d2d1ff7fa1183606f71a5c90f33263d9a78`. The combined artifact
`rust-release-candidate-140d7d2d1ff7fa1183606f71a5c90f33263d9a78` was downloaded independently.
Fresh release, recovery, and controlled-failure validation passed; regenerated release and recovery
manifests were byte-for-byte equal to CI's. The controlled evidence SHA-256 is
`c8a173b6adef05e0b655d09258bc92f08dcddec723863a88fcd595df700ffef7`.

The locked relay tests and strict targeted Clippy pass, including a unit regression for relay JSON
reformatting. A native build of the exact pinned relay reproduced the prior failure and passed the
corrected contract twice before clean shutdown. Final executable-path inspection found no HQ
daemon. No record accessor facade, storage shape or version, migration, compatibility reader,
duplicate stored/runtime state, dependency, or lockfile changed; no tag, release, soak, identity
activation, production service change, or cutover occurred.

### Original plan entry

- **[operations/high] Rehearse controlled relay and provider failure** — Dogfood the release
  candidate only with new identities and new state directories on controlled relays. Exercise
  startup, offline catch-up, relay loss and recovery, provider crash and drain behavior, and final
  clean shutdown with bounded, reproducible evidence and no production identity or live cutover.

## 2026-08-29 — Isolated identity and database recovery rehearsal

Added a release-binary recovery drill that owns a short, bounded `/tmp` rehearsal namespace and
supplies an explicit new state root to every HQ invocation. It creates canonical human and agent
state, runs explicit database repair, proves the authoritative projections remain equal, restarts
and stops the original node, waits for state ownership release, and round-trips a password-
encrypted identity into a new replacement root. The replacement proves exact public identity
equality while configuration and SQLite history remain absent, then starts with empty account and
agent history and shuts down cleanly.

The drill runs with a controlled temporary home containing inaccessible synthetic Go-layout key
and database sentinels. Those paths are never passed to HQ, every Rust state path lies outside the
layout, and their inode, mode, size, and modification/change metadata remains unchanged across all
product invocations. Recovery documentation now states plainly that the first release supports
identity backup and rebuildable-projection repair, not database-history backup/restore; an
identity-only replacement requires authorized relay/peer catch-up and must never overlap the
original identity on another live host.

Actions run 33252489386 passed the complete artifact and recovery sequence on Linux x86-64, Linux
ARM64, macOS x86-64, and Apple Silicon for exact revision
`6abdcf43f4820f1b55c19d3db55db1b78e647099`, then passed both aggregate validators. The combined
artifact `rust-release-candidate-6abdcf43f4820f1b55c19d3db55db1b78e647099` was downloaded
independently. Fresh release and recovery validation passed, and the regenerated recovery manifest
was byte-for-byte equal to CI's manifest. The validator's complete-matrix fixture passed and its
incomplete-repair fixture was rejected.

The locked full workspace passed formatting, architecture and qualification-inventory validation,
strict Clippy, and every all-target/all-feature test. Recovery scripts passed Bash syntax and
ShellCheck. Final exact executable-name inspection found no HQ daemon. No production record
accessor facade, storage shape or version, migration, compatibility reader, duplicate
stored/runtime state, dependency, or lockfile changed; no real Go or standing Rust state was
opened, and no tag, release, identity activation, or cutover occurred.

### Original plan entry

- **[operations/high] Rehearse isolated identity and database recovery** — Add a repeatable drill
  that uses only newly generated Rust identities and temporary state roots to prove encrypted
  identity export/import, backup boundaries, database repair, node replacement, restart, and clean
  shutdown. Prove that unsupported database-history restoration is described truthfully and that
  neither a Go key nor a Go database is opened or mutated.

## 2026-08-29 — Native Rust release artifacts

Replaced the write-enabled GoReleaser tag workflow with a manually dispatched, read-only Rust
release-candidate matrix. Each native runner builds the sole `hq` executable with its complete Git
revision embedded, packages an archive containing only that executable, emits a portable SHA-256
file, and publishes a target manifest only after an extracted installation initializes a new
identity, reaches ready state in an isolated state root, reports ready status, and shuts down
cleanly. Repository-owned validators reject unsafe names, nonnative hosts, missing or extra
targets, mixed versions or revisions, changed hashes, and absent lifecycle evidence. Their fixture
test proves both the complete four-target path and corrupted-archive rejection.

Actions run 33251594731 passed Linux x86-64, Linux ARM64, macOS x86-64, Apple Silicon, and the
aggregate verifier for exact revision `af7625225c4b41bf86c12d148a53e87755ac6e1f`. The native build
steps completed in 134, 100, 457, and 165 seconds respectively, all below the 900-second release
build limit. The combined artifact
`rust-release-candidate-af7625225c4b41bf86c12d148a53e87755ac6e1f` was downloaded independently;
a fresh aggregate audit verified all four archive hashes and reproduced the workflow manifest
byte-for-byte.

The locked full workspace passed formatting, architecture validation, strict Clippy, and every
all-target/all-feature test. The packaging scripts passed Bash syntax and ShellCheck. No HQ daemon
remained after local installation rehearsal or the test suite. No tag or release was published,
and no default, Go, or production identity/state path was opened. No production record accessor
facade, storage shape or version, migration, compatibility reader, duplicate stored/runtime state,
dependency, or lockfile changed.

### Original plan entry

- **[release/high] Build and verify native Rust release artifacts** — Replace the frozen Go release
  path with a Rust release-candidate workflow that builds one `hq` executable for Linux x86-64,
  Linux ARM64, macOS x86-64, and Apple Silicon. Package each target with checksums and a machine-
  readable manifest, prove that downloaded artifacts have the expected revision and host
  architecture, and rehearse installation, startup, and clean shutdown with a new identity and
  isolated state directory. Do not tag or publish a release as part of this task.

## 2026-08-29 — Cross-platform qualification and acceptance audit

Recorded the complete ADR-0001 native matrix for implementation revision
`762f0785059a87cf8c9bfeb34a6bd11bdc54de4a` and GitHub Actions run 33250739592. The combined
artifact contains the exact Linux x86-64, Linux ARM64, macOS x86-64, and Apple-Silicon environment
records. An independent download and replay of the repository validator accepted their common
revision, native host identities, complete budget set, exact record set, and clean release builds
of 95, 94, 184, and 165 seconds respectively against the 900-second limit.

Revalidated all eleven acceptance-inventory areas and the matrix validator's acceptance and
different-revision rejection paths. Reran the architecture gate, all eight installed PTY workflows,
the provider-neutral harness conformance suite, and the real Codex adapter seam. The audit found
direct current behavioral or configuration evidence for every integrated acceptance row, no
unexplained failure or exceeded budget, and no additional in-process, protocol, ownership,
recovery, or platform gap.

Updated the normative qualification document with the immutable run, implementation SHA, combined
artifact name, validator-produced platform table, installed lifecycle/provider scope, and
definition-of-done disposition. Operator-controlled relay/provider dogfood, backup/restore,
offline catch-up, failure and repair drills, node replacement, read-only Go archival, and rollback
rehearsal remain explicit in the next release-candidate task rather than being waived. No
production record accessor facade, storage shape or version, migration, compatibility reader,
duplicate stored/runtime state, dependency, or lockfile changed. The final exact executable-name
process audit found no HQ daemon.

### Original plan entry

- **[verification/high] Record cross-platform qualification and complete the acceptance audit** —
  Run and record the deterministic qualification commands and applicable installed lifecycle and
  provider evidence on the ADR-0001 Linux x86-64/ARM64 and macOS x86-64/Apple-Silicon matrix.
  Cross-check every acceptance-matrix row against direct current evidence, add any remaining gap
  back to the front of the queue rather than waiving it, and complete integrated qualification only
  when all required evidence is current and all quantitative budgets pass.

## 2026-08-29 — Bounded Linux workspace test-process lifecycles

Split the Ubuntu Rust suite into fourteen independently bounded crate owners while retaining the
aggregate macOS workspace suite and Ubuntu check, Clippy, and build gates. This preserved complete
coverage without serializing unrelated tests, and changed failures from a silent whole-runner
timeout into an exact crate, test, assertion, elapsed time, and retained terminal transcript.

The isolated failures exposed three independent lifecycle defects. TUI shutdown now uses an
out-of-band cancellation flag so queued client work cannot delay the shell's owning join. GNU
`kill` receives `--` before a negative process-group identity, so timed-out Git descendants and
their inherited output descriptors are terminated and reaped on Linux. Installed PTY tests now
synchronize durable agent and project mutations against authoritative snapshots rather than
assuming Ratatui's differential byte stream contains contiguous rendered phrases; the old raw
`Completed` and `revision ` checks could omit unchanged terminal cells and never send the exit key.

The exact final SHA passed the formerly flaky project lifecycle and agent-creation tests thirty
consecutive times each, the complete installed terminal target, strict package Clippy, and the
locked full workspace architecture, check, strict Clippy, all-target/all-feature test, and build
gates. CI runs 33250380569 and 33250517661 each completed all fourteen Linux owners successfully;
the first also completed the aggregate macOS Rust workspace successfully. The final exact
executable-name process audits found no HQ daemon. No production record accessor facade, storage
shape or version, migration, compatibility reader, duplicate stored/runtime state, dependency, or
lockfile changed.

### Original plan entry

- **[test/high] Bound Linux workspace test-process lifecycles** — The complete Ubuntu Rust suite
  reaches the job timeout twice after entering `cargo test`, retains the runner through its
  cancellation grace period, and publishes no log blob, while the same revision's native Linux
  qualification workloads and macOS workspace suite pass. Split the Linux suite into independently
  bounded owning groups, identify the test process or daemon retaining completion, and correct its
  lifecycle without serializing unrelated tests or weakening coverage. Complete this work when the
  full Linux workspace suite terminates normally on repeated runs, every spawned owner is reaped,
  and a failed test can still publish actionable diagnostics.

## 2026-08-29 — Integrated acceptance and performance gap closure

Audited every integrated acceptance row against the complete passing workspace and replaced
file-level evidence with exact checked proof selectors. Behavioral rows now name a concrete Rust
test function; scripts and the numeric budget source use closed command/configuration proof kinds.
The validator rejects malformed or renamed selectors, missing or untracked files, non-executable
commands, duplicate evidence, unknown proof kinds, incomplete rows, and unknown invocation modes.
A deliberate missing-selector check failed with the exact evidence path and function as intended.

The audit found direct current tests for generated algebra, authority races, protocol strictness,
durable failpoints and repair, indexed queries, local replay/restart, relay response loss, harness
partial persistence, project compensation, TUI/CLI parity and restoration, repair/restart recovery,
redaction, and bounded ownership. It found no unexplained behavioral failure, untested in-process
invariant, missing deterministic recovery scenario, or algorithmic regression. Native records on
the other ADR-0001 targets remain the next queued qualification work; controlled external relay,
provider, archival, dogfood, and operator recovery rehearsals remain explicitly queued for the
release candidate rather than being waived.

Serialized the three store performance workloads through a test-local lease and fixed the canonical
runner to one test thread. This removes workload-to-workload scheduler contention without changing
any measured region or production API. All explicit budgets pass, including the 40-second clean
release build. The locked full workspace passes formatting, check, strict Clippy,
all-target/all-feature tests and build, architecture, behavior-ledger, causal, protocol, dependency,
and both 512-run protocol-fuzz gates. No production record accessor facade, storage shape or
version, migration, compatibility reader, duplicate stored/runtime state, dependency, or lockfile
changed. The final exact executable-name process audit found no HQ daemon.

### Original plan entry

- **[verification/high] Close integrated acceptance and performance gaps** — Implement every
  missing test, recovery scenario, or algorithmic correction found by the checked evidence
  inventory. Strengthen and run the complete fixture, property, model, fuzz, crash/reopen,
  lifecycle, architecture, security/redaction, and end-to-end suites across the assembled node,
  clients, relay, harness, and project workflows. Meet the explicit cold-readiness, rebuild,
  late-parent/high-fanout, paging, invalidation-to-redraw, bounded-queue, memory, release-build, and
  graceful-shutdown budgets without unexplained failures, untested invariants, or hidden
  algorithmic regressions.

## 2026-08-29 — Reproducible qualification budgets and evidence mapping

Added a machine-checked inventory mapping all eleven integrated acceptance areas to direct fixture,
property, model, crash/reopen, lifecycle, architecture, security/redaction, fuzz, and end-to-end
evidence. Added one normative qualification contract and one canonical numeric budget file for cold
readiness, full rebuild, late-parent fanout, indexed later-page loading, invalidation-to-redraw,
saturated-queue shutdown, idle and active resident memory, clean release build time, and graceful
shutdown. The portable runner rejects missing inventory areas, unresolved evidence paths, and
unknown or malformed budget variables before running any workload.

New owning-boundary tests exercise a 1,002-fact rebuild, one late parent waking 500 dependants, ten
indexed later pages in a 1,000-entry conversation, invalidation of a ready 10,000-row pure UI model,
a saturated fixed-capacity local-session drain, and a real foreground node's readiness, RSS, status
work, stop, process join, and runtime-artifact cleanup. The runner executes these tests, builds the
single `hq` release executable in an isolated temporary target directory, enforces the checked-in
limits, and emits platform, Rust-host, revision, duration, and budget evidence. Native Linux and
macOS qualification is now a dedicated CI matrix; four-target portable checks remain explicitly
compilation evidence only.

Native Apple-Silicon qualification passed every budget, including a 40-second clean release build
against the 900-second limit. The locked full workspace passes formatting, check, strict Clippy,
all-target/all-feature tests and build, architecture, behavior-ledger, causal, protocol, dependency,
and both 512-run protocol-fuzz gates. No production record accessor facade, storage shape or version,
migration, compatibility reader, duplicate stored/runtime state, dependency, or lockfile changed.
The final exact executable-name process audit found no HQ daemon.

### Original plan entry

- **[verification/high] Establish reproducible qualification budgets and evidence mapping** — Turn
  every acceptance-matrix row into a checked evidence inventory covering the fixture, property,
  model, fuzz, crash/reopen, lifecycle, architecture, security/redaction, and end-to-end suites
  across the assembled node, clients, relay, harness, and project workflows. Define explicit,
  executable budgets and representative workloads for cold readiness, full rebuild,
  late-parent/high-fanout ingestion, long-conversation paging, invalidation-to-redraw, bounded queue
  behavior, idle/active memory, release build time, and graceful shutdown. Add deterministic
  qualification commands whose results can be recorded on every ADR-0001 target, and put every
  uncovered invariant or missing direct proof into the queue rather than treating documentation or
  compilation as evidence.

## 2026-08-29 — Retained project lifecycle controls

Added reopen, fresh close assessment, confirmed close, archive, and unarchive to project details.
Close first performs a read-only all-resource check through the ordinary local API, then retains
the exact accepted, rejected, or uncertain health and release evidence in a confirmation modal.
Confirmation and force recovery are separate choices. Only dirty, unknown, rejected, or uncertain
evidence requires force; clean and non-Git resources do not. Closing and archival explicitly retain
external paths, files, worktrees, branches, and uncertain runtime truth.

Every lifecycle action maps directly to the existing project CLI command through the local-client
boundary. The TUI adds no project authority and no alternate mutation route. Stable project
identity, fresh checks, confirmation, force choice, and submission state survive authoritative
reloads and reconnects. Existing typed operation identity, saga progress, runtime state and code,
rejection, reconciliation, compensation, and external-state warnings remain the sole outcome and
recovery evidence.

Pure-model coverage proves clean close, dirty force gating, separate confirmation, cancellation,
stable reload retention, reopen, archive, and unarchive. Wide and compact renderer tests cover
release evidence and lifecycle confirmations. Exact action-mapping tests cover every new local
command. Installed pseudoterminal coverage drives one project through close, reopen, archive, and
unarchive, checks each authoritative state through the CLI, and proves terminal restoration.

The locked full workspace passes formatting, check, strict Clippy, all-target/all-feature tests and
build, architecture, behavior-ledger, causal, protocol, dependency, and bounded protocol-fuzz
gates. No storage shape or version, migration, compatibility reader, protocol version, accessor
facade, duplicate stored/runtime state, dependency, or lockfile changed. The final exact
executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Implement retained project lifecycle controls** — Add confirmed close, force-gated
  takeover/close recovery, reopen, archive, and unarchive through ordinary local-API commands.
  Preserve project selection and modal state across authoritative reload/reconnect, reconcile
  in-flight operations by stable identity, and expose release assessment, quiescence, compensation,
  external-state warnings, rejected outcomes, and recovery actions as typed state. Complete this
  work when pure model, responsive renderer, executor, and installed-client parity tests cover every
  lifecycle transition, confirmation, cancellation, progress, response-loss, and failure path.

## 2026-08-29 — Retained project assignment, activation, dispatch, and handoff

Added passive public-field current-assignment and exact historical project-thread records to the
TUI project catalog. Project details render configuring, runnable, blocked, and cardinality-conflict
state. Stable local named-agent selection joins only authoritative project/agent/provider/session/
thread tuples; the broader agent session catalog is never treated as project resume authority.

Added new-session and exact-session activation, pending-input dispatch, and handoff through the
existing ordinary local-client project commands. Tab selects agent, mode, thread, provider,
directory, confirmation, and force fields without stealing printable form characters; arrows
change stable choices. Exact resume requires a matching project thread. Handoff requires a
separate confirmation, while force takeover remains independent authority and explicitly does not
claim external runtime cessation. Exact selections and edited text survive reload and reconnect.

Every project result now retains the existing typed runtime state and failure/uncertainty code in
addition to command identity, operation identity, saga stage, canonical head, rejection,
reconciliation, and external-state warnings. The TUI renders that evidence directly and never
imports project, provider, harness, filesystem, storage, or signing authority. No accessor facade,
duplicate stored/runtime state, storage migration, compatibility layer, protocol change, or
lockfile change was introduced.

Pure-model tests cover new and exact activation, stable reload retention, exact dispatch,
confirmation and force gates, cancellation through the common modal contract, stale response and
typed failure handling. Wide and compact renderer tests cover activation, blocked assignment,
handoff, takeover disclosure, and runtime uncertainty. Executor tests cover every exact action and
runtime evidence. Installed terminal coverage drives real pending dispatch and renders its typed
rejection; the installed CLI parity suite exercises activation, dispatch, handoff, restart, and
stale target failures. Full all-target/all-feature workspace tests, strict Clippy, formatting,
architecture, behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. The
final exact executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Implement retained project assignment, activation, dispatch, and handoff** — Add
  stable named-agent selection, new-session or exact-session activation, pending-input dispatch,
  and confirmed handoff/takeover through ordinary local-API commands. Preserve exact project,
  agent, provider/session, thread, and directory targets across reload/reconnect; require separate
  confirmation and force choices; and render saga progress, stale heads, runtime rejection,
  uncertainty, and recovery actions without importing project or provider authority into the TUI.
  Complete this work when pure model, responsive renderer, executor, and installed-client parity
  tests cover every assignment, activation, dispatch, handoff, cancellation, and failure path.

## 2026-08-29 — Retained desired-resource editing and conflict previews

Added exact desired-resource selection and retained add, replace, remove, and primary-selection
forms to project details. Arrow navigation and dedicated action keys preserve the selected resource
and every partially edited field across authoritative reload and reconnect. Cancellation does not
mutate state, while assigned-project removal requires a separate explicit force confirmation and
never deletes the external resource.

Add and replace now run an authoritative fresh preview through the ordinary local client before
mutation. The composition inspects the observed resource with the existing local-API command and
passes canonical observations through a narrow `hq-projects` domain seam, so the TUI receives only
the selected passive conflict and force policy. Clean previews can proceed with the retained
expected head; conflicting previews remain blocked and explain the existing claim and requested
relationship without importing resource authority into the TUI.

Fresh exact-resource and all-resource checks retain typed desired and observed paths, canonical
identity, health, release cleanliness or unknown state, operation identity, completed head,
runtime rejection, uncertainty, reconciliation guidance, and recovery actions. Every edit reuses
the established project workflow and expected-head rules. Passive records expose public fields;
no accessor facade, duplicate stored/runtime state, storage migration, compatibility layer,
protocol version, or lockfile change was introduced.

Pure-model tests cover each edit, preview, clean commit, conflict block, force gate, cancellation,
exact selection, retained form, exact and aggregate checks, retry, and failure path. Wide and
compact renderer tests cover all forms and conflict evidence. Executor and installed-client tests
preserve exact actions, perform a real add through the TUI, and verify the resulting CLI catalog.
Full all-target/all-feature workspace tests, strict Clippy, formatting, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. The final exact
executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Implement retained desired-resource editing and conflict previews** — Add desired
  resource add, remove, replace, and primary selection plus fresh health/release checks through
  ordinary local-API commands. Present domain-selected claim conflicts and force gates before
  mutation, retain exact project/resource selection and modal inputs across reload/reconnect, and
  expose stale heads, dirty/unknown release state, rejected outcomes, and recovery actions as typed
  state. Complete this work when pure model, responsive renderer, executor, and installed-client
  parity tests cover every resource edit, preview, cancellation, conflict, and failure path.

## 2026-08-29 — Retained project catalog, creation, and input TUI workflows

Added stable-identity project search, selection, and responsive details to the pure Ratatui model.
Search covers names, resource paths, and full project identities. Logical selection, search text,
open details, exact project identity, and partially edited creation or input forms survive
authoritative reorder, reload, reconnect, and resize. Escape cancels only when no command is in
flight and performs no mutation.

Added typed existing-working-tree creation, recoverable Git worktree provisioning, and project
input effects. Forms retain every field after client failure. Matching effect and exact action
identity are both required before a completion can replace the modal, so stale or cross-wired
responses fail visibly. Accepted/running progress, completed heads, rejection category/code,
reconcilable stage, and retained external destination/branch warnings remain typed passive state
with actionable recovery guidance.

The ordinary local-client composition validates bounded names, content, project identities, paths,
branches, and bases, then reuses the existing CLI/local-API project authority and saga workflows.
The TUI layer receives only passive public-field project/resource records and typed outcomes; it
does not import project, filesystem, Git, messaging, storage, or signing authority. No accessor
facade was added to passive records, no stored/runtime state was duplicated, and no storage or
local-API version, migration, or compatibility shape changed.

Pure-model tests cover catalog reorder, reload/reconnect/resize retention, both creation modes,
input, cancellation, client failure, progress, stale-head rejection, response mismatch, and
reconcilable response loss. Wide and compact renderer tests cover worktree forms and external-state
recovery. Executor tests preserve exact actions and operation evidence. Installed pseudoterminal
tests create an existing-tree project, provision a real exact-base Git worktree and branch, send
project input, and restore terminal modes. Full all-target/all-feature workspace tests, strict
Clippy, formatting, architecture, behavior-ledger, causal, protocol, protocol-fuzz, and dependency
gates pass. The final exact executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Implement retained project catalog, creation, and input workflows** — Add stable
  project search, selection, and details before accepting new project work; create projects over an
  existing working tree; provision recoverable Git worktrees; and send project input through
  ordinary local-API commands. Preserve modal text and logical project selection across
  authoritative reload, reconnect, and resize; cancel without mutation; reject stale heads; and
  expose worktree progress, rejected outcomes, reconcilable external-state warnings, and recovery
  actions without recomputing project authority. Complete this work when pure model, responsive
  renderer, executor, and installed-client parity tests cover catalog, both creation modes, input,
  cancellation, progress, response loss, and failure paths.

## 2026-08-29 — Retained managed-session TUI lifecycle

Added provider-neutral start, exact resume, and stop commands to named-agent details. Start accepts
an explicit provider, exact resume binds the highlighted durable provider/session identity, and
stop retains durable session history. Starting while a durable session is selected or resuming a
different session requires a conservative confirmation that explicitly avoids treating durable
selection or the runnable catalog flag as live-process evidence.

The pure model now retains each section's logical selection, focus, open conversation, technical
disclosure, and conversation anchor while visiting Agents. Stable managed-session operations stay
pending across reconnect and resize; stale completions are ignored. Ready, stopped, rejected, and
uncertain outcomes retain exact operation evidence, rejection category/code, or reconciliation
identity with corrective actions. Passive action and result records expose public fields directly;
no accessor facade or duplicate stored/runtime state was introduced.

The bounded executor maps exact typed effects through the ordinary local-client composition into
the existing harness CLI workflow. That workflow continues to capture launch directory and copied
environment outside the TUI, allocate one retry-safe operation identity, and use the existing
ordinary AgentSession local-API frame and response-loss reconciliation. The TUI imports no domain,
provider, harness, storage, filesystem, or process authority and never stores or renders launch
environment.

Pure model and responsive rendering cover start, exact resume, stop, switch cancellation,
reconnect/resize persistence, stale completion, rejected and uncertain outcomes, and mailbox
workspace restoration. Executor tests preserve exact targets and operation evidence. The installed
pseudoterminal exercises explicit-provider start and typed rejection, while installed CLI restart
coverage exercises stop and stale exact resume. Full workspace tests, strict Clippy, formatting,
architecture, behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. No
storage or local-API version, migration, or compatibility shape changed. The final exact
executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Implement retained managed-session lifecycle** — Add start, exact resume, live switch
  confirmation, and stop for one stable named-agent/provider/session target through the ordinary
  local API. Keep durable provider/session selection separate from runtime presence, retain
  mailbox editing/navigation state while managing sessions, reconcile stable in-flight operations
  after reconnect, and expose stale, rejected, or uncertain outcomes as typed actionable state.
  Complete this work when pure model, responsive renderer, executor, and installed-client tests
  cover every retained session lifecycle use case without importing provider or domain authority
  into the TUI.

## 2026-08-29 — Retained named-agent TUI catalog and administration

Added stable-identity named-agent search and inspection to the pure Ratatui model. Search covers
agent identities, permanent names, providers, durable sessions, and display names; its query,
logical agent selection, open details, and exact provider/session selection survive authoritative
reorder, reload, reconnect, and resize. Responsive wide and compact overlays expose lifecycle,
runnable selection, mailbox count, durable sessions, conflicts, and resolved names without parsing
display prose.

Added typed create, session rename/clear, and permanent retirement actions. Retirement requires an
explicit confirmation, exposes force as a separate visible toggle, and Escape cancels without
mutation. Failed, stale, conflicted, retired, rejected, or potentially uncertain commands preserve
the exact modal inputs for correction. Passive agent, mailbox, session, action, and modal records
use public fields; only the invariant-bearing UI model and effect identity retain accessors.

The snapshot mapper reuses the existing named-agent catalog reduction, preserving exact validated
provider/session identities as action targets while sanitizing presentation names. The bounded
executor carries exact typed effects through the ordinary local-client composition and existing
CLI/local-API workflows, which retain their stable request and response-loss behavior. The TUI
executor imports no domain, planner, project, provider, signer, storage, or process capability.

Pure model, responsive render, exact executor mapping, full workspace all-target/all-feature,
installed TUI/CLI create and daemon-restart parity, strict Clippy, format/check, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. This package added no
storage or local-API version, migration, compatibility facade, duplicate stored/runtime record, or
passive-record accessor layer. The final exact executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Implement retained named-agent catalog and administration** — Add stable-identity
  named-agent search and inspection plus create, rename, and confirmed retirement through ordinary
  local-API commands. Preserve logical agent selection, search, and open details across
  authoritative reload, reconnect, and resize; cancel administration modals without mutation;
  reconcile in-flight commands after reconnect; and expose stale, conflicted, assigned, retired,
  rejected, or uncertain outcomes as typed actionable state. Complete this work when pure model,
  responsive renderer, executor, and installed-client tests cover every retained catalog and
  administration use case without importing agent or project authority into the TUI.

## 2026-08-29 — Retained TUI mailbox composition and actions

Added pure typed reply, direct-message, self-note, archive, and restore interactions to the
Ratatui model. Responsive direct-target selection, draft loading and composition, and archive or
restore confirmation overlays preserve focus, logical selection, scroll anchors, stable targets,
and modal state across authoritative reload, reconnect, and resize. Activity entries cannot become
message-action targets, and cancelling a confirmation performs no canonical mutation.

Draft composition supports exact UTF-8 typing, paste, backspace, a 16 KiB bound, coalesced
autosave, save-before-close, and save-before-submit. Editing while a save is in flight triggers a
second save for the latest text before close. Optimistic conflicts preserve local text while
adopting the authoritative version, stale targets retain recoverable draft content, and only a
committed mailbox receipt closes and consumes a submitted draft.

The terminal shell and bounded effect executor now run draft and mailbox effects through the
ordinary local API. Stable effect and request identities reconcile response loss without giving
the TUI storage, planner, provider, or domain authority. Direct-message candidates are derived
authoritatively from active named agents with one mailbox, and the real installed TUI self-note
flow produces the same canonical message as the CLI and survives daemon restart.

Passive mailbox targets, drafts, actions, and modal data expose public fields directly. Only the
invariant-bearing model and effect identity retain accessors. The unshipped storage schema and
local API were used as-is: this package added no version bump, migration, compatibility shape, or
duplicate stored/runtime state type.

Pure transition, renderer snapshot, executor, terminal normalization, installed pseudoterminal,
restart, full workspace all-target/all-feature, strict Clippy, format/check, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. The final exact
executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Implement retained mailbox composition and actions** — Add pure reply,
  direct-message, self-note, archive, and restore interactions over the ordinary mailbox-command
  service. Preserve applicable draft identity, focus, target reselection, modal state, logical
  selection, and scroll anchors across authoritative reload, reconnect, and resize; cancel modals
  without mutation; and reconcile in-flight effects by stable request identity. Complete this work
  when responsive render, model, executor, actionable-error, stale-target, modal-cancellation, and
  installed TUI/CLI parity tests cover every retained mailbox interaction without selected-row or
  activity-target leakage.

## 2026-08-28 — Durable local drafts and mailbox commands

Added typed installation-local reply, direct-message, and self-note drafts with stable identities,
bounded content, explicit targets, optimistic versions, autosave/load/delete operations, and
restart persistence. Empty drafts remain recoverable while canonical message submission still
requires nonempty content. Stale message and agent targets deliberately remain attached to their
draft text instead of being removed by foreign-key cleanup.

Added reply, direct-message, self-note, archive, and restore commands to the ordinary local API.
The node resolves the local human authority, exact message/thread or agent target, and current
archive frontier from one transaction-consistent canonical snapshot. Stable request digests bind
the command and submitted content, byte-identical requests replay their receipt after response
loss, and changed requests fail explicitly. A successful draft-backed command deletes the draft in
the same transaction as the canonical mutation and receipt; rejection preserves it for correction.

The unshipped storage schema and local API evolved directly in place. No storage or protocol
version was bumped, and no migration, compatibility reader, accessor facade, or duplicate stored
draft/runtime record was introduced. Passive draft and mailbox-command records expose public
fields; existing invariant-bearing identifiers and bounded canonical content retain their narrow
constructors.

Store contracts cover every draft target, optimistic conflicts, capacity bounds, restart,
stale-target preservation, authoritative direct/reply resolution, exact receipt replay, and changed
request identities. A transaction failpoint after draft consumption proves that draft deletion,
canonical fact insertion, and receipt insertion roll back together. Reconnecting-client tests lose
the first command response and replay the exact frame; installed CLI coverage proves mailbox
commands and delivery state survive daemon restart.

Full workspace all-target/all-feature tests, strict Clippy, format/check, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. The dependency audit
retains only its allowed duplicate-version trees and existing yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[mailbox/high] Add the durable local draft and mailbox-command service** — Add typed
  installation-local reply, direct-message, and self-note drafts with stable identities, bounded
  content, explicit targets, autosave/load/delete operations, and restart persistence. Expose draft
  operations plus reply/send/archive/restore through the ordinary local API, resolve every target
  and causal frontier authoritatively in the node, reconcile commands by stable request identity,
  and consume a submitted draft atomically with its canonical mutation receipt. Preserve stale
  targets for recovery rather than deleting their text. Complete this work when store failpoint,
  restart, replay, changed-request, stale-target, and CLI parity tests pass without a storage
  migration or compatibility reader.

## 2026-08-28 — Pure Ratatui model/effect architecture and renderer

Added a pure `hq-tui` transition algebra in which the complete `UiModel` and one closed `UiEvent`
produce a new model plus ordered, identity-bearing `UiEffect` values. Startup, normalized input,
resize, timers, authoritative snapshots, revision-only invalidations, and generation-scoped
connection observations perform no I/O or domain mutation. Nonzero effect identities suppress late
snapshot successes, failures, and timer completions; invalidations coalesce to the greatest required
revision and force a follow-up snapshot when an in-flight response cannot satisfy it. Logical row
identity preserves selection across reload and resize.

Added borrowed Ratatui rendering for wide navigation/content, compact tab/content, and bounded
undersized-terminal layouts. The header exposes section, connection, and revision; rows expose
selection and typed state in text as well as style; the footer exposes controls or an actionable
failure. Exact terminal-buffer snapshots cover all three layouts and assert that rendering leaves
the entire model unchanged.

The crate has no internal HQ dependency and owns no terminal, clock, runtime, local transport,
storage, filesystem, process, provider, or domain capability. Passive sizes, rows, snapshots,
failures, and transition results use public fields without constructors or accessor facades.
Only `UiModel` and `EffectId` stay opaque because they enforce outstanding-effect, revision,
generation, selection, startup, and nonzero-identity invariants. The architecture verifier now
enforces this boundary, while `docs/rust/tui.md`, the workspace contract, and behavior-ledger
`CLI-013` record the stable shell interface. Terminal ownership, reconnect execution, input
decoding, local-API mapping, and RAII restoration remain in the next composition package.

Full workspace all-target/all-feature tests, strict Clippy, format/check, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. Ratatui introduces two
reported but allowed transitive duplicate-version trees; the existing yanked `chacha20 0.10.1`
warning also remains. The final exact executable-name process audit found no HQ daemon.

### Original plan entry

- **[tui/high] Build the pure Ratatui model/effect architecture and renderer** — Implement
  `UiModel`, the closed `UiEvent` enum, pure update transitions, explicit identity-bearing
  `UiEffect` values, stale effect-response suppression, borrowed rendering, and responsive layout.
  Add exhaustive model/effect tests and deterministic buffer snapshots across representative
  terminal sizes. Complete this work when state transitions and rendering perform no I/O or domain
  mutation and expose a stable shell boundary.

## 2026-08-28 — Recoverable worktree provisioning and non-TUI parity

Added `project worktree NAME --source ABSOLUTE_PATH --destination ABSOLUTE_PATH --branch BRANCH
[--create-branch BASE] [--brief TEXT] [--home INSTALLATION_ID]` through the existing durable
project saga. Existing-branch mode requires the exact named branch; creation mode validates the
exact base commit and creates the branch from it. The selected home—not the calling client—observes
the source, base, destination, common repository, branch, resource identity, and advisory claim,
so the same command works locally or through signed remote-home routing.

Reservation, Git intent/result, resource identification, and canonical project creation remain
separate durable checkpoints. Reconciliation proves the exact destination, common repository, and
branch before continuing. A typed `worktree-may-exist` warning now survives workflow storage,
local API transport, CLI human/JSON output, remote canonical signing, reducer projection, and
restart whenever Git may have retained external state. HQ never removes or rewrites that worktree
or branch automatically, including after rejection, close, or archive.

The unshipped application, control protocol, local API, and storage schema evolved in place. No
version bump, migration, compatibility shape, accessor facade, or duplicate stored/runtime record
was introduced. Worktree requests, warning views, and other passive records expose public fields;
only invariant-bearing values retain constructors and accessors.

Fake-port tests prove reservation ordering, incoherent branch-mode rejection before effects,
response-loss repair, exact replay, canonical rejection warnings, and no duplicate Git mutation. A
real-Git test loses a successful creation response, reconstructs the workflow manager from its
durable checkpoint, and repairs by lookup without a second mutation. Real-Git adapter tests prove
exact older-base creation, invalid-base and branch conflicts, existing-branch mode, symlink safety,
stale registration handling, and repository serialization. Foreground CLI coverage provisions the
exact older commit, restarts the daemon, preserves the project/resource identity, and proves close
and archive leave both worktree and branch intact.

The retained non-TUI behavior-ledger audit found and closed one additional gap: a bare `hq` without
a terminal now lists the human inbox, while pure `parse_cli([])` still renders help and terminal
selection remains available to the queued Ratatui package. Stale Go-era `--no-sync` prose was
removed because Rust mutations send no implicit wake; `hq relay sync` is the explicit prompt.

Full workspace all-target/all-feature tests, strict Clippy, format/check, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. The dependency audit
retains only the existing yanked `chacha20 0.10.1` warning, and the final exact executable-name
process audit found no HQ daemon.

### Original plan entry

- **[cli/high] Add recoverable worktree provisioning and audit non-TUI parity** — Expose exact Git
  worktree provisioning with destination reservation, source/base/branch validation, optional
  branch creation, local or remote home, durable progress, reconciliation, and orphaned external
  state warnings. Add fake-node and real-Git response-loss/restart tests, then audit every retained
  non-TUI behavior-ledger workflow and close any remaining CLI gap.

## 2026-08-28 — Desired-resource mutation commands

Added `project resource add`, `remove`, `replace`, and `primary` through the stable project-command
path. Add and replace carry a caller-allocated stable resource identity plus a normalized display
locator; the immutable home identifies the canonical locator and current health before the exact
expected-head mutation can commit. Replacement receives a fresh stable identity, primary selection
names an existing identity directly, and assigned-resource removal still requires a separate
`--force` authority.

The unshipped application, saga codec, and local API vocabulary evolved in place. No storage or API
version was bumped, and no migration, compatibility record, accessor facade, or duplicate
stored/runtime type was introduced. Resource command values remain passive public-field records;
filesystem observation and overlap policy stay behind the resource port and canonical reducer.

Pure workflow and codec coverage proves normalized inputs, changed identification, overlap and
identity conflicts, resource-specific stale-head rejection before mutation, assigned removal force,
primary selection, response-loss replay, and exactly-once canonical effects. A real foreground test
adds and selects a resource, rejects a cross-project nested overlap, replaces and removes resources,
restarts the daemon, and verifies stable remaining identity. Marker files prove add, replace,
remove, close, and archive never delete or modify external paths.

Full workspace all-target/all-feature tests, strict Clippy, format/check, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. The dependency audit
retains only the existing yanked `chacha20 0.10.1` warning, and the final executable-name process
audit found no HQ daemon.

### Original plan entry

- **[cli/high] Add desired-resource mutation commands** — Expose resource add, remove, replace, and
  primary selection through the stable project-command path. Preserve stable resource identities,
  exact expected heads, and explicit force semantics. Test overlap conflicts, stale heads, response
  loss, restart, and no external deletion or mutation on close, archive, or remove.

## 2026-08-28 — Desired-resource inspection and fresh checks

Added snapshot-only `project resource list PROJECT_ID` and `project resource show PROJECT_ID
RESOURCE_ID` plus fresh `project check PROJECT_ID [RESOURCE_ID]` commands. Passive public-field
views expose stable resource identity, normalized display and immutable canonical locators, primary
selection, projected and freshly observed health, advisory claims, every overlap conflict, Git
release state, observation time, rejection, response loss, and reconciliation identity. Duplicate
resource identities fail closed.

Fresh checks issue stable, digest-bound requests through the existing read-only application port in
resource-ID order. The digest binds operation/time, project/resource identity, and both locators;
the server rejects a mismatched body. Checks never modify filesystem, Git, desired membership, or
claims. Because v1 resources are home-machine scoped, a non-home check fails closed instead of
observing an unrelated local path namespace. The unshipped local API v1 was evolved in place with
no version bump, migration, compatibility shape, accessor facade, or duplicate stored/runtime
record.

Parser, deterministic human/JSON presentation, duplicate/conflict, digest, server rejection, and
home-selection tests pass. A real foreground test creates a project through a symlink into a clean
Git worktree, observes clean then dirty release state, retargets the symlink and observes degraded
identity plus an unknown release gate, restarts the daemon, and proves the stable resource identity
survives. Full workspace all-target/all-feature tests, strict Clippy, format/check, architecture,
behavior-ledger, causal, protocol, protocol-fuzz, and dependency gates pass. The dependency audit
retains only the existing yanked `chacha20 0.10.1` warning, and the final process-table audit found
no HQ daemon.

### Original plan entry

- **[cli/high] Add desired-resource inspection and check commands** — Expose complete desired-resource
  inspection and fresh health/release checks through the project and read-only inspection ports.
  Identify display/canonical locators at the boundary, preserve stable resource identities, and
  show primary selection, health, conflicting projects, and dirty/unknown release gates. Test
  symlink/path changes, overlap conflicts, restart, and real foreground execution.

## 2026-08-28 — Project handoff and takeover CLI

Added `project handoff` through the stable project-command path. The command resolves exactly one
current assignment plus the target named agent, provider, historical thread, optional exact session,
launch directory, immutable project home, active account, and current head from one authoritative
snapshot. It requires `--yes` for every handoff; `--force` remains separate takeover authority for
blocked or uncertain quiescence and cannot substitute for confirmation.

Handoff reuses activation's exact session/thread and primary-directory validation, then sends the
existing closed `ProjectCommandAction::Handoff` and renders the complete accepted/running/completed,
rejected, runtime failure/uncertainty, or reconcilable outcome. No domain policy was copied into the
CLI: same-agent, busy/retired/threadless target, blocked takeover, forced uncertainty, stale-head,
response-loss, and restart behavior remain owned and exhaustively tested by the pure workflow.

Parser, confirmation, force separation, authoritative assignment/target resolution, foreground
threadless rejection, full handoff workflow, response-loss recovery, and restart tests pass. Full
workspace all-target/all-feature tests, strict Clippy, format/check, architecture, behavior-ledger,
causal, protocol, and dependency gates pass. The dependency audit retains only the existing yanked
`chacha20 0.10.1` warning, and the final process-table audit found no HQ daemon. The change uses the
existing unshipped local API v1 and storage v13 shapes without a version bump, migration,
compatibility layer, accessor facade, or duplicate stored/runtime record.

### Original plan entry

- **[cli/high] Add project handoff and takeover commands** — Reuse the authoritative assignment and
  historical-thread views to hand a project to an exact agent, provider, session, thread, and
  launch directory. Require explicit confirmation, keep forced takeover separate from ordinary
  handoff, and render every workflow stage, rejection, runtime failure/uncertainty, and reconcilable
  operation. Test same-agent and busy-target rejection, threadless targets, blocked and forced
  takeover, stale heads, response loss, restart repair, and real foreground execution.

## 2026-08-28 — Project activation and pending-dispatch CLI

Added `project activate` with explicit fresh-session or exact-session/thread selection and `project
dispatch` for pending accepted inputs. Activation resolves the named agent, provider, immutable
project home, current head, exact session binding, historical project thread, and launch directory
from one authoritative snapshot. Exact resume requires the complete
agent/provider/session/project/thread tuple; fresh sessions may optionally continue one historical
thread. The sole authoritative primary resource supplies the default launch directory, while an
explicit absolute directory remains subject to home-side claim validation.

Extended the application projection and unshipped local API v1 in place with passive public-field
assignment and deduplicated historical-thread records. Assignment phase is explicit and independent
of current runnability, preserving cardinality and claim conflicts without an accessor facade. No
storage schema, version bump, migration, compatibility path, or duplicate stored/runtime type was
added. Project catalog human and JSON output now includes current assignment phase, runtime binding,
blocking state, support, and historical session/thread provenance.

The production foreground harness boundary now converts a missing provider or exact session into a
typed rejected runtime effect, allowing the project workflow to compensate and render its normal
terminal rejection instead of leaking a generic adapter error. Parser, invalid session/thread,
exact binding, primary directory, projection deduplication, protocol phase invariants, stable command
identity, stale-head, response-loss, workflow recovery, and real-process restart tests pass. Full
workspace all-target/all-feature tests, strict Clippy, format/check, architecture, behavior-ledger,
causal, protocol, and dependency gates pass. The dependency audit retains only the existing yanked
`chacha20 0.10.1` warning, and the final process-table audit found no HQ daemon.

### Original plan entry

- **[cli/high] Add project activation and pending-dispatch commands** — Extend the stable
  project-command builder with activation, exact-session/thread resume, and pending dispatch.
  Resolve the named agent, provider, launch directory, active assignment, and historical thread
  bindings from authoritative state; render every workflow stage, rejection, runtime
  failure/uncertainty, and reconcilable operation. Test invalid session/thread combinations, stale
  heads, response loss, restart repair, and real foreground execution.

## 2026-08-28 — Project lifecycle CLI

Added `project open PROJECT_ID`, `project close PROJECT_ID --yes [--force]`, `project archive
PROJECT_ID`, and `project unarchive PROJECT_ID`. Every lifecycle command resolves the selected
active account, immutable project home, and exact canonical head from one authoritative snapshot,
then submits through one stable project-command builder. Close always requires explicit `--yes`;
`--force` is separate authorization for dirty/unknown release or failed/uncertain runtime cessation
and cannot substitute for confirmation.

Added the application-to-local-API project request conversion boundary and reused it for creation
and lifecycle commands, removing the CLI's hand-built creation DTO. The conversion is exhaustive
over the closed project action catalog and round-trips lifecycle values. Existing outcome rendering
now serves every lifecycle command and preserves accepted/running stages, terminal rejection,
reconcilable checkpoints, and exact runtime failure or uncertainty details. Passive records retain
public fields; no accessor facade or duplicate stored/runtime state was introduced.

Parser, confirmation, exact authority/head digest, conversion round-trip, stale-head precondition,
byte-identical response-loss replay, close/archive recovery, runtime failure/uncertainty rendering,
and real foreground CLI lifecycle tests pass. The foreground test closes and archives one project,
restarts its owning daemon, unarchives and opens it, proves desired resource identity survives while
advisory claims release and reacquire, and explicitly stops the owner. Full locked workspace tests
with all targets/features, format/check, strict Clippy, architecture, behavior-ledger, causal,
protocol, and dependency gates pass. The dependency audit retains only the existing yanked
`chacha20 0.10.1` warning. Repeated post-suite process-table audits found no HQ daemon. Local API
remains v1 and storage remains v13; no migration or compatibility path was added because HQ has not
shipped.

### Original plan entry

- **[cli/high] Add project lifecycle commands** — Expose open, close, archive, and unarchive through
  one stable project-command builder shared with subsequent assignment commands. Resolve the active
  human, immutable home, and exact expected head from authoritative state; require explicit close
  confirmation and force authorization; render every workflow stage, rejection, runtime
  failure/uncertainty, and reconcilable operation. Test stale heads, response loss, restart repair,
  close confirmation, and real foreground execution.

## 2026-08-28 — Project-addressed messaging CLI

Added `project send PROJECT_ID [MESSAGE]` through the ordinary asynchronous-message planner. The
command accepts one bounded argument or stdin body, resolves the immutable project account and
mailbox from one authoritative snapshot, requires exact active human authority, and emits stable
human and `hq-cli-output-v1` message/project identities. Account conversation addressing now permits
a direct recipient only when paired with a typed project ID; partial, mismatched, and cross-account
forms fail closed.

Added a bounded home input reconciler that selects usable unaccepted project messages in stable fact
order and authors contiguous `ProjectInputAccepted` facts with exact previous-state, project-home,
and account-membership authority. It runs after committed local mutations, replicated ingest, and
startup recovery. Acceptance uses the message's signed timestamp plus deterministic randomness,
command ID, and digest, so response loss and restart replay the exact plan. Closed, archived, and
unassigned projects retain sequenced pending work; dispatch remains a separate runnable-assignment
decision.

Passive project/message projections and clean relational rows now carry the immutable account,
mailbox, account scope, and signed message time needed for planning. These unshipped storage and
local-API shapes changed in place: storage remains v13 and local API remains v1, with no migration,
compatibility reader, accessor facade, or duplicate stored/runtime state type.

Parser, stdin, strict addressing, causal authority, cross-account rejection, deterministic output,
stable response-loss identity, store reopen, sequencing, restart, and real daemon tests pass. Full
locked workspace format/check/build/tests/doctests/strict-Clippy, architecture/behavior/causal/
protocol gates, dependency policy, four-target compilation, both 512-run fuzz smokes, and frozen Go
vet/build/tests pass. The dependency audit retains only the existing yanked `chacha20 0.10.1`
warning. Two complete process-owning CLI-suite runs finished normally, and the final process-table
audit found no HQ daemon.

### Original plan entry

- **[cli/high] Add project-addressed messaging** — Add `project send` through the ordinary
  application message planner so closed or unassigned work remains pending for the authoritative
  home. Test message sequencing, stdin, causal authority, response loss, restart, and deterministic
  human/machine output.

## 2026-08-28 — Isolated process-owning CLI integration fixtures

Eliminated the intermittent full-suite wait and orphaned test-daemon symptom by serializing
process-owning integration fixtures within each test binary. The shared `TestDirectory` now owns a
reentrant thread-qualified lease: one test may still create several installations and exercise
real concurrent callers, while unrelated tests cannot overlap daemon generations or inherit one
another's captured process streams. The production launcher already sent daemon stdin/stdout/stderr
to null, so no product lifecycle behavior or storage/protocol shape changed.

A regression test proves another test thread cannot acquire a process fixture until the owner
releases it. The complete 17-test real CLI binary now finishes under the ordinary parallel harness
without intervention, including concurrent readiness, process races, multi-installation pairing,
restart, and explicit stop. The full locked workspace suite and strict node-test Clippy pass, and a
post-suite process-table audit finds no HQ daemon.

### Original plan entry

- **[runtime/high] Eliminate intermittent concurrent-autostart output waits** — Reproduce the full
  parallel CLI-suite case where a readiness caller waits on inherited output after its single
  expected daemon is ready. Make process spawning and child release guarantee that background node
  generations cannot retain an invoking CLI's output pipes or survive an explicit test stop.
  Stress concurrent readiness with other autostarting commands, bound completion, and assert the
  process table and runtime artifacts are clean afterward.

## 2026-08-28 — Existing-resource project creation CLI

Added `project create NAME --path ABSOLUTE_PATH [--brief TEXT] [--home INSTALLATION_ID]` through
the typed local API and project workflow. Creation allocates stable command-derived workflow,
project, mailbox, and resource identities, binds the exact request digest, carries no fabricated
previous head, and identifies the existing directory on the authoritative home before committing
one initially open project with a healthy primary claim. Passive application, protocol, and CLI
records expose public fields; invariant-bearing identities, locators, and live capabilities remain
validated or opaque.

Remote creation now uses the same durable request/receipt/outcome path as existing-project control.
The clean canonical and rebuildable projection shapes carry optional expected and received heads,
so a remote home can acknowledge creation before the project exists and can still author a definite
rejection. Committed outcomes bind the new canonical head. These unshipped protocol and clean-schema
changes were made in place: storage remains v13 and local API remains v1, with no migration,
compatibility reader, or accessor facade.

Parser, help, request identity, canonical body, workflow replay, remote routing, response-loss,
nullable projection round-trip, stale-home, path identity, concurrent real-process claim,
machine-output, and restart tests pass. Formatting, locked workspace tests/build, strict Clippy,
architecture, protocol, causal, behavior-ledger, dependency, and bounded protocol-fuzz gates pass.
The full parallel CLI suite exposed one intermittent inherited-output wait in its pre-existing
concurrent-readiness test; stopping that test's single expected daemon allowed the suite to finish,
and the focused test immediately passed cleanly. A final process-table check found no HQ daemon.

### Original plan entry

- **[cli/high] Add project creation over an existing resource** — Extend the project workflow port
  with exact identification of an existing resource, then add `project create` with local or
  selected remote home, deterministic mailbox/project identity, and exact no-head replay. Test
  response loss, concurrent or changed creation, stale home authority, restart, path identity, and
  machine output without adding a storage/local-API version or migration.

## 2026-08-28 — Authoritative project catalog CLI

Added local-API-only `project list` and `project show PROJECT_ID` commands to the installed Rust
executable. Each command autostarts or connects to the node, reads one fresh complete authoritative
snapshot, and produces passive public-field project records in stable identity order. Human and
`hq-cli-output-v1` JSON output expose immutable home, lifecycle/archive state, head and input
sequence, desired resources, health, primary and active-claim state, every claim conflict, accepted
input/dispatch/output attribution, and structured remote-command receipt, result, and runtime
checkpoints.

The projection joins dispatches only through exact accepted-message identity and outputs only
through exact dispatch identity. Missing attribution is counted explicitly; duplicated identities
and project-owned rows without a project fail closed. Conflicted lifecycle, claims, dispatches,
outputs, and remote progress remain visible without selecting a winner. No storage or local API
version, migration, compatibility accessor, or parallel persistence record was added.

Strict parser/help, deterministic rendering, heterogeneous incomplete/conflicted projection, exact
show failure, architecture, and real foreground restart tests pass. Full locked workspace tests,
build/check, formatting, strict Clippy, architecture, behavior/causal/protocol specifications,
dependency policy, four portable targets, both bounded fuzz smokes, and frozen Go build/vet/tests
pass. The dependency audit retains only the existing yanked `chacha20 0.10.1` warning, and the
post-suite process table contains no HQ daemon.

### Original plan entry

- **[cli/high] Expose the authoritative project catalog and remote progress** — Add local-API-only
  `project list/show` commands over the complete snapshot. Present lifecycle, archive state, head,
  input sequence, resources, primary/active claims, conflicts, input/dispatch/output attribution,
  and structured remote-command checkpoints without choosing through inconsistent state. Test
  parsing, deterministic human/JSON output, incomplete/conflicted projections, restart, and
  architecture isolation.

## 2026-08-28 — Managed harness CLI workflow

Added explicit provider-neutral `harness start`, exact `resume`, and `stop` commands to the installed
Rust executable. Every command resolves one active named agent and crosses only the authenticated
local API. Start and resume canonicalize the caller's launch directory and copy the complete caller
environment at that boundary, preserving non-UTF-8 values as sensitive binary data. One random
operation identity and a digest over the exact agent, provider, action, time, directory, and
environment make transport replay byte-identical without causing separate invocations to alias.

Human and `hq-cli-output-v1` records expose ready, stopped, rejected, and uncertain outcomes without
launch secrets. Exact resume never creates a replacement session; rejection exits 1, while explicit
uncertainty returns immediately with the operation and reconciliation identities and exits 3. The
node still owns provider workers, and the CLI autostarts it through the existing coordinator.

Storage now consumes the neutral `HarnessSessionOperation`, kind, and state records directly. The
identical `StoredHarnessSessionOperationState` family and its exhaustive node conversion layer were
deleted; capability-bearing owner tokens remain encapsulated. This is an in-place cleanup of the
unshipped storage shape with no migration, compatibility type, accessor facade, or version bump.

Parser, exact-identity, binary-environment, redaction, uncertainty, response-loss, architecture,
machine-output, stale-resume, restart, and real daemon tests pass. Full locked workspace tests,
build/check, formatting, strict Clippy, architecture, behavior/causal/protocol specifications,
dependency policy, four portable targets, both bounded fuzz smokes, and frozen Go build/vet/tests
pass. The dependency audit retains only the existing yanked `chacha20 0.10.1` warning, and the
post-suite process table contains no HQ daemon.

### Original plan entry

- **[runtime/high] Expose the managed `hq harness` client workflow** — Add local-API-only CLI start,
  exact-resume, and stop commands that resolve one named agent, copy the caller environment and
  absolute launch directory at the boundary, derive stable exact request identity, autostart the
  node, and render ready/stopped/rejected/uncertain outcomes. Test parsing, response loss, stale
  sessions, non-UTF-8 environment values, node restart, machine output, and architecture isolation.

## 2026-08-28 — Continuous live harness event draining

Added a bounded supervisor polling pass that visits every live worker in stable agent order and
polls each provider source once without blocking. Normalized output retains its stable output
identity; activity derives a deterministic checkpoint identity from operation, item, kind, logical
key, runtime, and semantic sequence. Complete normalized values determine checkpoint digests, and
snapshot status alone enters exact operation/logical-key coalescing.

Each worker now owns its fixed persistence FIFO, one explicit just-polled staging slot, and a
separately bounded source-ordered interactive-request queue. A full FIFO cannot lose the event
already returned by the provider: that value remains staged and its source is not polled again
until admission succeeds. Existing snapshots may still replace their exact pending predecessor,
while distinct durable values preserve FIFO order. Persistence outages leave accepted memory work
owned; exact session resume and stable canonical identities recover provider replay without
duplicate facts. Normal closure and typed poll failure both tear down only the exact worker and
release its lease with redacted failure evidence.

`HarnessNodeComponent` now owns one named joined event thread for the complete supervisor. Launch
and delivery wake it immediately; ordinary work uses an explicit poll interval. Shutdown first
closes every provider's intake, unparks the task independently of that interval, continues bounded
provider polling, joins the task, flushes normalized work, drains or force-stops sessions, and only
then releases worker ownership. A real component test uses a 60-second interval to prove launch
wake, ordered drain, and zero retained task/supervisor ownership.

Deterministic recovery tests cover source order, snapshot replacement under persistence outage,
FIFO saturation plus the staging slot, exact replay after restart, provider closure, typed provider
failure, partial checkpoints, and diagnostic redaction. The successful Unix CLI integration suite
also exposed three generated-state daemons that its tests did not stop; those cases now perform
explicit teardown, and the post-suite process table is clean. Full locked workspace format, check,
strict all-feature Clippy, tests, build, architecture, dependency, and bounded protocol fuzz gates
pass (the dependency audit retains its pre-existing yanked `chacha20 0.10.1` warning). No storage
or local-API version changed, and no migration, compatibility representation, or passive-record
accessor facade was added.

### Original plan entry

- **[runtime/high] Continuously drain live harness event streams** — Poll every live provider worker
  through bounded component-owned runtime work, normalize source-ordered output/activity into the
  supervisor buffer, and drive canonical persistence without losing backpressured or restartable
  work. Test restart recovery, buffer saturation and coalescing, provider closure/failure, ordered
  shutdown, and zero leaked worker/task ownership.

## 2026-08-28 — Canonical normalized harness value authoring

Added pure application planners for normalized harness output and activity. Their passive request
and authority records expose public fields, while fact construction retains the existing
invariant-owning plan boundary. Output is authored from the exact active agent mailbox to the
installation-local human with its operation correlation and presentation; activity preserves the
complete normalized provider, session, item, logical-time, runtime, sequence, status, content, and
truncation values.

Added a node-owned persistence adapter that revalidates the exact live agent, mailbox, installation,
provider, session, and either direct-session or runnable project-assignment authority inside each
canonical transaction. Deterministic command identities make equal replays idempotent and reject
changed reuse of a stable output or activity identity. Causal support includes the authority roots,
binding evidence, and prior activity frontier, so stale or conflicted bindings fail closed. The
adapter maps output before activity through the supervisor's existing independent checkpoints,
allowing recovery after partial persistence without duplicating either fact.

Foreground composition now gives the neutral harness component the canonical adapter through the
same waking application store used by other node-owned persistence. Unit and recovery tests cover
exact correlation and normalized fields, direct and project bindings, duplicate values, stable-ID
collisions, partial output-before-activity recovery, stale authority, and redacted diagnostics.
Full locked workspace format, check, strict all-feature Clippy, tests, build, architecture,
dependency, and bounded protocol fuzz gates pass. Storage and local API were evolved in place: no
version bump, migration, compatibility record, or accessor facade was introduced.

### Original plan entry

- **[runtime/high] Author normalized harness values as canonical facts** — Implement pure planning
  and a node-owned persistence adapter that idempotently commits normalized output and activity
  under the exact active agent mailbox, provider, session, and operation correlation. Preserve
  output-before-activity checkpoints and reject stable-identity collisions or stale bindings.
  Test duplicate values, partial persistence, stale authority/session state, and redacted failures.

## 2026-08-28 — Foreground Codex worker composition and launch policy

Registered the concrete Codex factory only in the foreground node composition root. The node owns
provider-private executable, model, permissive execution, timeouts, process grace, frame capacity,
and durable-agent developer instructions. Launch resolution requires an active uniquely named
agent plus an existing absolute working-tree directory; invalid paths and retired agents fail
closed before child creation. Passive configuration and runtime request records expose public
fields, while the resolver and process/session owners remain opaque capabilities.

Project activation now carries its already-selected launch directory through the neutral passive
runtime request to the harness. Stop-only requests leave it absent. This preserves the deliberate
ordering in which runtime readiness precedes canonical launch revalidation without manufacturing
runnable projection state or adding accessors. Direct managed sessions continue using the node's
canonicalized directory and copied environment.

Codex characterization and conformance tests cover exact start/resume identity, copied environment,
diagnostic redaction, and bounded process teardown. New node/project tests prove exact directory
propagation, foreground-only registration and private options, invalid launch rejection, and a real
restart that drains one complete foreground ownership graph before reopening a fresh generation.
The architecture gate now requires the node-owned Codex dependency and foreground registration;
provider vocabulary remains absent from neutral crates. Full locked workspace format, check,
strict all-feature Clippy, tests, build, architecture, dependency, and bounded protocol fuzz gates
pass. No storage or local-API version changed and no migration or compatibility layer was added.

### Original plan entry

- **[runtime/high] Compose foreground Codex workers and provider launch policy** — Register the
  Codex adapter only in the foreground node composition root and resolve validated launch
  directories, executable, developer instructions, and provider-private execution options there.
  Test exact start/resume, environment and diagnostic redaction, process teardown, real foreground
  restart behavior, and the dependency boundary that keeps Codex vocabulary out of neutral crates.

## 2026-08-28 — Managed named-agent session lifecycle reconciliation

Added provider-neutral start, exact-resume, and stop as a dedicated retry-safe local API family.
The reconnecting client retains and replays the exact encoded frame after response loss or explicit
uncertainty, shares changed-identity detection with every retryable command family, and completes
with a definite result or returns explicit uncertainty for caller-visible reconciliation. Both
client and server recompute a canonical digest
covering operation/time, agent/provider/action, launch directory, and every copied environment
entry. Binary environment values use canonical base64; secret-owning environment fields remain
opaque, redact diagnostics, enforce strict bounds, and zero values on drop.

Added a durable managed-session operation ledger across the neutral supervisor and clean-sheet v13
store schema. Prepared operations checkpoint uncertainty before provider I/O; ready, stopped, and
rejected states are absorbing; equal replay is idempotent; changed reuse collides; and restart
observation never guesses through unresolved runtime state. The deterministic injected provider
proves exact readiness replay, changed identity, missing resume uncertainty, worker ownership, and
restart repair. Environment values and provider diagnostics never enter storage.

The node now validates the active installation-local named agent and canonical absolute launch path
before provider work. Only the exact acknowledged provider session can then author its immutable
mailbox binding, repository context, and complete-frontier selection through deterministic,
replay-safe canonical mutations. Passive request/state/result records expose public fields. Storage
now consumes the same neutral operation, kind, and state records directly; the former identical
stored copies and their exhaustive record-only node mapping were unnecessary indirection and are
gone.

Because HQ has never shipped and has no standing installations, local API v1 and storage v13 were
evolved directly in place with no protocol bump, storage-version bump, migration, or compatibility
scaffolding. Full locked workspace tests, strict all-feature Clippy, architecture, dependency,
behavior-ledger, causal/spec, protocol-spec, and bounded fuzz gates pass.

### Original plan entry

- **[runtime/high] Reconcile managed named-agent session lifecycle** — Add retry-safe neutral start,
  exact-resume, and stop commands over the local API; securely copy the caller environment and
  launch directory only at the control boundary; durably reconcile request identity, readiness,
  uncertainty, and restart repair; and bind, contextualize, and select only the exact acknowledged
  session. Exercise the workflow with an injected provider and test stale sessions, resume
  mismatch, response loss, runtime uncertainty, redacted diagnostics, restart recovery, and
  in-place storage/API evolution without compatibility scaffolding.

## 2026-08-28 — Mailbox messaging and repository-aware discovery

Added `ask`, `send`, `wait`, `poll`, `get`, `mailboxes`, and human `list`, `answer`, `cancel`,
`archive`, and `restore` commands over pure application planners and typed local API requests.
Passive Rust command, request, result, context, and diagnostic records use public fields; only
invariant-owning plans, identities, and live clients remain opaque. Explicit provider/session
selection requires both values, environment discovery rejects ambiguity, and repository-aware
selection joins canonical directory, repository, and worktree observations to direct sessions.

Message pages now carry typed purpose, presentation, correlation, project association, reversible
state, and normalized thread readiness. Inspection is non-consuming, asynchronous sends return a
stable message identity immediately, and waits are intentionally unbounded unless requested while
each local I/O attempt remains bounded. Ready delivery writes stdout before applying a reversible
archive completion, giving duplicate-safe at-least-once behavior across interruption. Missing
causal history is exposed as bounded inert diagnostics and cannot grant reply, cancellation,
completion, or state-change authority.

Because HQ has never shipped and has no standing installations, the clean local API v1 and storage
v13 contracts were completed in place with no migration, compatibility path, protocol bump, or
storage-version bump. Pure planner, strict protocol, relational projection, restart/reconnect,
incomplete-history, stale-state, non-TTY input, filter, deterministic machine-output, repository
discovery, and real stdout-before-archive CLI tests pass together with the locked workspace and
architectural verification gates.

### Original plan entry

- **[cli/high] Implement mailbox messaging and repository-aware discovery** — Add `ask`, `send`,
  `wait`, `poll`, `get`, human list/filter, answer, cancel/archive, restore, and repository-aware
  mailbox discovery over typed application/local API operations. Preserve stable message identity,
  causal reply/cancellation authority, non-consuming inspection, duplicate-safe ready delivery,
  asynchronous send, and intentionally unbounded human wait with bounded per-attempt I/O. Support
  explicit session mailbox selection without ambiguous provider inference. Test restart, reconnect,
  incomplete history, duplicate delivery, stale targets, non-TTY input, filters, and machine output.

## 2026-08-28 — Relay synchronization, health, and repair administration

Added `relay add|list|remove|sync|status|repair` over typed application capabilities and local API
requests. Policy changes retain exact stable effect identities, equal desired state reconciles as an
unchanged no-op, removal durably disables instead of deleting history, targeted synchronization
rejects absent or disabled policies, and accepted/rejected/uncertain outcomes retain an audit
identity in deterministic human and JSON output. Status reports a bounded sorted policy set,
generation and enabled state, durable queue/delivery counts, and explicit truncation.

State health reports decision and conflict counts for the stable authority, conversation, agent,
and project catalog. The store actor pairs revision and normalized index in one serialized request;
explicit repair reverifies immutable evidence, atomically replaces only rebuildable state, and
returns the repaired index at that same serialization boundary. Policy capacity and active-session
bounds fail closed. Passive application, wire, and CLI records expose public fields; invariant-owning
identities and live capabilities remain opaque.

Because HQ has never shipped and has no standing installations, the clean local API v1 and storage
v13 contracts were completed in place with no migration, compatibility path, or version bump.
Application/store/protocol contracts, stable-identity and stale-revision tests, real CLI
add/sync/repair/disable/restart/redaction coverage, and the two-installation fake-relay response-loss
scenario pass together with locked workspace tests, strict Clippy, build, architecture, protocol,
causal, behavior-ledger, and dependency gates.

### Original plan entry

- **[cli/high] Implement relay policy, synchronization, health, and repair administration** — Add
  relay add/list/remove, explicit sync, domain/delivery health status, and explicit repair commands
  over typed local effects and authoritative observations. Preserve stable effect identities,
  accepted/rejected/uncertain reconciliation, bounded relay policy, offline queues, prompt-wake
  semantics, and repair as an explicit audited operation. Test response loss, restart, disabled and
  incompatible relays, stale revisions, offline rendering, redaction, and end-to-end fake-node
  coverage.

## 2026-08-28 — Directional peer and mailbox capability administration

Added `peer add|list|distrust` and `mailbox list|grant|revoke` through public-field application
requests, authoritative local API snapshots, and the existing exact mutation client. Route history
is directional and complete: inspection retains every set, block, causal maximum, public key,
label, and non-authority relay hint without choosing a conflicted winner. Mailbox inspection retains
the exact grant fact, installation-qualified mailbox and grantee signing address, revoke frontier,
observed actions, and complete support.

Exact current routes and capabilities reconcile as no-ops. Route recovery cites the full block
frontier; mailbox regrant cites the complete prior revoke lineage and creates a distinct stable
grant. Distrust commits every active capability revoke before its route block, and explicit outbox
fanout preserves the grantee recipient after projection changes. Missing ownership, route conflict,
ambiguous capability history, stale or partial authority, concurrent revoke/action, and later use of
an old grant fail closed under the existing reducer laws.

The clean unshipped local API v1 and storage v13 definitions were completed in place. Storage now
retains each exact capability grant fact; because HQ has never shipped and has no standing
installations, there is no migration, compatibility path, protocol bump, or storage-version bump.
Pure planner, strict DTO, relational codec/corruption, parser/help, deterministic JSON, fanout,
response-replay, causal race, and real two-installation add/grant/distrust/recover/regrant/restart
tests pass. Locked workspace build/tests, strict Clippy, architecture, protocol, causal,
behavior-ledger, and dependency gates pass; dependency policy reports only the existing allowed
yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[cli/high] Implement directional peers and mailbox capabilities** — Add peer add/list/distrust
  and mailbox grant/revoke/inspection commands over exact application plans and authoritative
  snapshots. Keep route trust directional and distinct from mailbox authority; preserve historical
  observations, revoke-before-block delivery ordering, full installation-qualified addresses, and
  fail-closed concurrent/later authorization. Test stale frontiers, replay, block recovery, relay
  hints as non-authority, and local-API-only architecture.

## 2026-08-27 — Pinned Codex app-server adapter

Implemented the Codex provider boundary against pinned official CLI `0.150.1` schemas and fixtures. The adapter owns a bounded stdio JSON-RPC process, exact start/resume/read and turn lifecycle behavior, durable-submission reconciliation, supported fail-closed server requests, normalized output/activity, redacted typed failures, and graceful-to-forced shutdown. Passive configuration uses public fields while process and protocol invariants remain opaque. A real adapter seam passes the neutral 14-scenario conformance suite; deterministic transport/process/fixture tests, an opt-in installed-provider start/resume smoke, all locked workspace gates, four supported targets, fuzzing, dependency policy, and unchanged Go gates pass.

### Original plan entry

- **[codex/high] Implement and pin the Codex provider adapter** — Select a current supported Codex
  app-server baseline using official schema/documentation and installed-binary evidence, pin its
  generated fixtures, and privately implement process startup, bounded JSONL/JSON-RPC transport,
  initialization, exact thread start/resume/read behavior, turn start/steer/interrupt,
  stable-submission reconciliation, supported server requests, additive notification tolerance,
  normalized output/activity, typed failure causes, stderr trust boundary, and shutdown escalation.
  Keep every Codex DTO and method name out of neutral crates. Complete this work when the neutral
  conformance suite, pinned protocol fixtures, process tests, and opt-in installed-provider smoke
  test pass.

## 2026-08-26 — Durable installation-local TUI drafts

Added unsigned, installation-local SQLite drafts with optimistic versions and wire-7 RPC/client operations. The TUI now restores drafts before its first render, coalesces serialized autosaves over a documented 250 ms abrupt-loss window, requires successful persistence before stow or graceful quit, explicitly deletes canceled or emptied drafts, and retains bodies on failures and stale targets for reselection. Atomic submission uses the stable draft UUID as message identity, commits normal replies or project inputs with draft consumption in one transaction, preserves project activation intent, restores target wakes on RPC replay, and prevents duplicate messages after lost responses. Store, RPC, client, TUI, restart, stale-target, debounce-order, rollback, project-input, replay, full-suite, vet, build, and focused race tests pass.

### Original plan entry

## Durable installation-local TUI drafts

- Add unsigned `tui_drafts` storage and domain/wire-7 DTOs for `TUIDraft`, `ListTUIDrafts`,
  `PutTUIDraft`, `DeleteTUIDraft`, and `SubmitTUIDraft`. Persist a stable UUID, optimistic
  version, body, reply target/conversation or new-message recipient, address/label, repository
  context, domain-level project activation intent, and timestamps. Drafts never become
  canonical/Nostr or replicated state.
- Load drafts before first render. Preserve current draft rows and resume behavior. Autosave active
  edits after a 250 ms coalescing window, serialize saves with optimistic versions, and force a
  successful save before stowing or graceful quit; failed saves keep the editor open with an error.
- Use the stable draft UUID as message/idempotency identity. `SubmitTUIDraft` durably commits the
  message or pending project input and consumes the draft as one recoverable mutation. Lost replies
  retry without duplication; failed sends retain the draft; emptying explicitly deletes it.
- Keep stale-target drafts visible and require recipient/project reselection rather than dropping
  their bodies. Test TUI/daemon restart, invalidation reload, stale targets, ordered debounce,
  failure retention, successful consumption, and the documented abrupt-loss debounce window.

## 2026-08-24 — Durable agent and project work reconciliation

Added a durable pending-work projection for direct named-agent inboxes and runnable project assignments, including persisted selected threads and launch directories. The supervisor now runs an observer-safe initial scan plus coalesced invalidation and periodic repair scans, reusing existing worker/waking guards and automatic resume validation while treating explicit RPC wake environments as optional latency hints. Node startup installs the observer before reconciliation begins. Store, supervisor, restart, exclusion, invalidation, duplicate-trigger, full-suite, vet, and race tests cover convergence without a second message.

### Original plan entry

## Durable agent and project work reconciliation

Replace best-effort RPC wake calls with a supervisor reconciliation loop driven by durable pending work. This depends on canonical project-input acceptance so the supervisor has one reliable source of truth.

Scope:

- Add store queries that describe runnable pending work for direct named agents and project assignments.
- Reconcile pending work after the supervisor and store observer are installed during daemon startup.
- Reconcile on relevant message, project, assignment, and delivery invalidations.
- Start or resume offline workers when durable selected-thread and repository state make them runnable.
- Treat the sending client's environment as an optional launch hint, not a correctness dependency.
- Coalesce concurrent invalidations and make reconciliation idempotent with existing worker and waking guards.
- Retain explicit wake calls only as latency optimizations, or remove them if reconciliation is immediate enough.
- Handle pending work committed by remote ingestion, startup repair or rebuild, direct store callers, and a process crash after commit but before RPC wake.

Primary areas:

- `internal/codexsupervisor/supervisor.go`
- `internal/node/node.go`
- `internal/domain/codex_runtime.go`
- `internal/domainrpc/server.go`
- Project and named-agent store query implementations
- `internal/codexsupervisor/supervisor_test.go`
- Node integration tests

Risks:

- Reconciliation must not launch archived, closed, unassigned, retired, or otherwise non-runnable targets.
- Startup ordering must avoid losing invalidations between the initial scan and observer registration.
- Repeated scans must not create duplicate workers or duplicate dispatches.

Acceptance criteria:

- Restarting the daemon alone eventually delivers already-accepted work to a runnable offline project assignment.
- Remote messages wake eligible offline project and direct-agent workers without a local Create or Reply RPC.
- A fault injected after commit but before explicit wake converges after restart.
- Running workers are not duplicated, and non-runnable projects or agents remain offline.
- Existing selected thread and working-directory state are honored after restart.

Implementation plan:

- Add a focused durable pending-work query that returns one launchable target per direct named-agent mailbox or runnable project assignment, including the selected Codex thread and persisted launch directory, only when incomplete delivery exists.
- Give the supervisor an idempotent reconciliation loop with a buffered trigger and periodic repair scan. Start it explicitly after node change-observer installation, trigger it from message/project/agent invalidations, and run an initial scan so startup cannot miss already committed work.
- Feed durable targets through the existing automatic wake paths, preserving their worker and `waking` guards, last-known-good launch configuration, persisted thread selection, project binding validation, and local daemon environment fallback.
- Keep explicit RPC wake calls as optional low-latency/environment hints; correctness comes from the durable scan, including relay ingress, direct store calls, rebuild, and commit-before-wake crashes.
- Add store query tests for direct/project eligibility and exclusion states, supervisor startup/invalidations tests for direct and project work, and duplicate-trigger tests proving one worker launch and eventual dispatch.

Risks and decisions:

- Runtime ownership leases can briefly outlive a crashed daemon. Periodic reconciliation must retry pending targets after lease expiry rather than treating the initial ownership conflict as terminal.
- The observer is installed after supervisor construction so subscribers can safely receive synchronous store publications; the node must install it before starting the initial reconciliation scan.
- A durable scan supplies the daemon environment only when no in-memory last-good request or explicit wake hint exists; environment remains transient and is never stored in the pending-work DTO.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-24 — Mandatory project runtime contracts

Added a daemon-only composite project runtime store contract spanning core operations, project mutations, delivery, output, commands, workflows, and durable pending work. The supervisor now requires that contract and no longer discovers mandatory capabilities at runtime. Codex bridge project delivery and output dependencies are explicit and narrowly scoped, while direct-mode and RPC client interfaces remain small. Added compile-time assertions for SQLite, the RPC client, and supervisor runtime roles, and removed mandatory-capability fallback branches and type assertions.

### Original plan entry

## Mandatory project runtime contracts

Make core project-runtime dependencies compile-time requirements while retaining small optional interfaces only at genuine extension boundaries. Do this after the domain refactors so the final required capabilities are known.

Scope:

- Define a composite daemon-side project runtime store contract covering the project, workflow, delivery, output, command, and pending-work capabilities required by the supervisor and Codex bridge.
- Change node and supervisor constructors to require the appropriate contract instead of accepting generic `domain.Operations` and discovering mandatory capabilities through type assertions.
- Give bridge components focused required interfaces where project mode cannot function without them.
- Keep client-facing and test-double interfaces narrower; do not force local SQLite-only delivery internals onto the RPC client.
- Add compile-time interface assertions for SQLite, the RPC client, supervisor, and other concrete implementations against their intended contracts.
- Replace "capability unavailable" runtime branches for mandatory daemon wiring with construction-time failures.

Primary areas:

- `internal/domain/store.go`
- `internal/domain/projects.go`
- `internal/codexsupervisor/supervisor.go`
- `internal/codexbridge`
- `internal/node/node.go`
- `internal/hqclient`

Risks:

- One oversized interface would make unit tests cumbersome; split contracts by consumer and compose them only at the node boundary.
- Truly optional runtime controllers must remain optional where degraded operation is intentional.

Acceptance criteria:

- The daemon cannot compile or start with a store missing mandatory project delivery or recovery capabilities.
- SQLite and each RPC or client implementation have explicit compile-time assertions for the contracts they own.
- Mandatory project execution paths contain no runtime type assertion whose failure indicates an internal wiring error.
- Unit-test fakes remain focused and readable.

Implementation plan:

- Define a daemon-only `ProjectRuntimeStore` in `internal/domain` by composing the existing general, project mutation, workflow, delivery, output, command, and pending-work interfaces; keep the public `Store` and RPC-client contract unchanged.
- Require `ProjectRuntimeStore` in the supervisor constructor, collapse its optionally discovered project/workflow/pending fields into the required store, and remove mandatory-capability nil checks and type assertions from activation, close, handoff, retirement, worktree, remote-command, recovery, and reconciliation paths.
- Split Codex bridge wiring into a required named-mailbox store plus an explicit focused project bridge store. Pass project delivery/output capabilities only for project workers, and remove project-mode assertions from dispatch and output publication.
- Add compile-time assertions for SQLite's daemon contract, the RPC client's public store/runtime/provisioning contracts, and the supervisor's runtime/controller contracts; let compile failures expose incomplete test doubles at their actual consumer boundaries.
- Run focused bridge/supervisor/node tests, then repository-wide vet, tests, and race-enabled project-runtime tests.

Risks and decisions:

- The daemon contract is intentionally broad only at the supervisor/node composition root; bridge subcomponents continue to accept narrow interfaces so direct-mode fakes do not acquire project-only methods.
- Project bridge dependencies remain explicit option fields because direct named-agent bridges legitimately do not need project delivery or output capabilities.
- RPC clients must not implement daemon-local delivery, workflow, or pending-work internals; their existing public store assertion remains separate from the new daemon-only contract.

## 2026-08-24 — Project ingress and delivery conformance suite

Added a reusable project conformance fixture and behavioral matrices for local create/reply, canonical append/replay/rebuild/startup repair, all typed message purposes, human/agent/home/replica destinations, and runnable/unassigned/closing/closed/archived project states. Added remote reorder/replay/restart convergence coverage, the previously missed project Reply followed by an offline daemon restart and automatic worker dispatch, and an SQLite-backed RPC reply integration test that proves semantic acceptance, assignment-bound claimability, and replay idempotency. The full normal, vet, repeated-focus, and race-enabled repository suites pass.

### Original plan entry

## Project ingress and delivery conformance suite

Build a reusable matrix and invariant suite that exercises the complete project-message lifecycle across every ingress and runtime state. Begin fixtures while implementing the preceding capabilities, then make the full suite the final integration gate.

Coverage matrix:

- Ingress: local Create, local Reply, remote canonical append, replayed mutation, canonical rebuild, and startup recovery.
- Destination: human mailbox, direct named agent, home project, and replica project.
- Project state: open and runnable, open and unassigned, closing, closed, and archived.
- Worker state: running, offline, daemon restarted, and worker launch already in progress.
- Message purpose: conversational input, structured protocol answer, project output, and notice.

Assertions:

- Canonical message identity, threading, reply relationship, and original-message archive behavior.
- Exactly one project acceptance for each eligible input and none for ineligible purposes or destinations.
- Deterministic acceptance sequence and project-head progression.
- Durable wake or reconciliation and eventual dispatch to the selected thread.
- No duplicate worker, claim, dispatch, or protocol delivery.
- Correct mailbox kind, label, device attribution, panel badge, and correlated presentation source.

Fault and invariant tests:

- Crash after canonical commit but before explicit wake.
- Restart after acceptance but before dispatch.
- Duplicate and reordered remote canonical delivery.
- Unknown project event and command operations.
- Malformed typed payloads.
- Rebuild from canonical history with empty derived project tables.
- Every acceptance references a projected canonical message.
- Every eligible home-project input has exactly one acceptance.
- Unsupported replica events do not advance the replica head.
- Every supported event and command is registered in all required projections and handlers.
- Every mailbox kind has a typed display mapping.

Primary areas:

- `internal/store/projects_test.go`
- `internal/store/sqlite_test.go`
- `internal/codexsupervisor/supervisor_test.go`
- `internal/codexbridge/*_test.go`
- `internal/domainrpc/server_test.go`
- `internal/node` integration tests
- `internal/tui/tui_test.go`

Acceptance criteria:

- The matrix includes the previously missed Reply x home project x offline or restarted worker combinations.
- RPC tests assert semantic acceptance and dispatch, not only that a mocked method was invoked.
- Fault tests demonstrate eventual convergence without a second user message or manual resume.
- The full suite passes under normal and race-enabled test runs.

Implementation plan:

- Add a reusable store conformance fixture that can inject project-addressed messages through local Create, local Reply, generic canonical append, duplicate replay, rebuild, and startup reconciliation, then assert the shared invariants: one projected message, one acceptance, a canonical message reference, deterministic sequence, and stable project head.
- Add table-driven destination, message-purpose, and project-state coverage. Verify only eligible human conversation addressed to a home project is accepted, every lifecycle preserves acceptance, and only an open runnable assignment is dispatchable.
- Add duplicate/reordered replica-history coverage proving deterministic convergence and preservation of the last valid head, complementing the existing unknown/malformed reducer tests and typed event/command completeness tests.
- Add the specific Reply → home project → daemon restart/offline worker regression test, asserting startup reconciliation launches one worker and dispatches the reply to the assignment's selected thread without a second wake message.
- Add an RPC integration test backed by SQLite that performs a real project reply mutation and asserts canonical acceptance plus assignment-bound claimability, rather than stopping at mocked method invocation.
- Treat existing bridge delivery crash-window tests, supervisor recovery tests, TUI mailbox/presentation exhaustiveness tests, and typed registry completeness tests as matrix rows; run full normal, vet, and race-enabled suites as the final integration gate.

Risks and decisions:

- The coverage matrix is compositional rather than a wasteful Cartesian product: each axis is behaviorally exercised, while high-risk intersections (Reply/home/restart and remote/replay/rebuild) receive dedicated end-to-end cases.
- Canonical replay and startup fixtures deliberately enter below convenience methods so they exercise the same repair boundaries used after crashes and upgrades.
- Worker tests use the scripted app-server protocol already used by supervisor tests, keeping the suite deterministic and network-independent.

## 2026-08-24 — Canonical project-input acceptance invariant

Moved project-input acceptance into the canonical ingest boundary, where one source-agnostic reconciler sequences every eligible human conversation addressed to an authoritative project. Local create/reply, generic appends, remote append, relay receive, startup, and rebuild now converge through the same transaction; the specialized project-message writer and version-specific reply repair were removed. Structured protocol answers remain excluded by typed purpose, closed-project notices stay atomic with unique acceptances, project invalidations include remotely accepted inputs, and recovery/replay tests prove exactly-once behavior.

### Original plan entry

## Canonical project-input acceptance invariant

Centralize project input acceptance as a canonical commit invariant instead of attaching it separately to `Create`, `Reply`, remote append, and repair code paths. This requires typed message purpose so structured protocol answers can be excluded deliberately.

Scope:

- Introduce one transactional reconciliation function invoked after canonical projection whenever messages enter or are replayed.
- Apply it uniformly to local create, local reply, remote `AppendCanonical`, mutation replay, database rebuild, and startup recovery.
- Guarantee that every eligible conversational human input to a home project mailbox has exactly one `project.message.accepted` event and acceptance row.
- Preserve deterministic per-project sequence ordering and correct project-head advancement.
- Keep closed and archived pending notices as part of the same invariant.
- Remove entry-point-specific project detection and the version-specific `repairLocalProjectReplies` path once general reconciliation covers it.
- Ensure retries, duplicate canonical events, and concurrent ingress remain idempotent.
- Make canonical rebuild reconstruct authoritative project acceptance state rather than passing because local project tables were retained.

Primary areas:

- `internal/store/sqlite.go`
- `internal/store/project_inbound.go`
- `internal/store/project_delivery.go`
- `internal/store/projects.go`
- SQLite schema and migrations
- `internal/store/projects_test.go`
- `internal/store/sqlite_test.go`

Risks:

- Acceptance creation itself appends a canonical project event; reconciliation must avoid recursion and double sequencing.
- Project-head compare-and-swap and multiple accepted messages must remain atomic.

Acceptance criteria:

- No eligible projected project input can commit without exactly one matching acceptance.
- No acceptance can exist without its referenced canonical message.
- Replaying or re-appending the same canonical data creates no additional acceptance or sequence.
- Create, Reply, remote append, recovery, and rebuild produce equivalent project state.
- The specialized reply repair is no longer required.

Implementation plan:

- Replace source-specific acceptance helpers with one `reconcileProjectInputsTx` pass over all projected human messages addressed to authoritative local project mailboxes, filtered exclusively by typed conversational/project-input purposes.
- Run reconciliation after canonical projection in the common local append transaction, remote `AppendCanonical`, and explicit rebuild/startup recovery. Let the reconciler sign one acceptance at a time, rebuild authoritative projection, and continue in deterministic project/message order until no eligible gap remains.
- Refactor `Create` and `Reply` to normalize local project conversation purpose before signing but use the generic canonical append boundary; remove `createProjectMessage`, inbound-only filtering, and startup `repairLocalProjectReplies`.
- Keep closed/archive pending notices within reconciliation and make notice generation idempotent by tying it to the unique acceptance event.
- Add invariant checks and tests covering local create, local reply, remote append, rebuild/recovery, duplicate replay, structured protocol exclusion, closed-project notices, and deterministic sequencing.

Risks and decisions:

- Reconciliation appends canonical project events while already inside a canonical-ingest transaction. Guard the common append helper against recursive reconciliation and always recompute pending inputs from the newly rebuilt projection.
- Rebuild may need to append missing acceptance events, so startup rebuild must use the local signer and commit both canonical additions and the final projection atomically.
- Existing acceptance events remain authoritative; reconciliation fills only missing eligible messages and never resequences or replaces established history.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-24 — Authoritative reducer-driven project projection

Made canonical project events the sole authority for local project projections during mutation and rebuild. The shared reducer now retains current and historical resources, every claim and assignment epoch, execution threads, acceptances, and dispatches; new runnable events carry complete thread facts, while legacy resource and thread details are bridged only from existing projections with explicit diagnostics when unavailable. Rebuild now recreates normalized project tables, preserves valid operational saga state, stops safely on forks, updates project mailbox labels, and local mutation, acceptance, and dispatch paths no longer duplicate projection SQL. Added schema migration and clean-rebuild, lifecycle, legacy-compatibility, fork-safety, full-suite, vet, and race coverage.

### Original plan entry

## Authoritative reducer-driven project projection

Make the typed reducer the source of authoritative SQLite project state during live mutation, replay, and clean rebuild.

Scope:

- Extend reducer output to retain project resources, claim epochs, assignment history, execution threads, message acceptances, and dispatch records needed by normalized SQLite tables.
- Include sufficient execution-thread facts in new assignment events, with deterministic compatibility handling for existing histories.
- Rebuild authoritative home projects and their normalized child tables from canonical project history rather than retaining mutable tables.
- Remove local mutation SQL that duplicates reducer application; after canonical ingest, read the reducer-built projection.
- Preserve operational leases and saga records only where they are intentionally noncanonical, and reconcile them against rebuilt authority.
- Preserve fork safety by stopping at the last unambiguous authoritative child.

Acceptance criteria:

- Local mutation, canonical replay, replica projection, and clean rebuild apply the same reducer semantics.
- A clean rebuild recreates projects, assignments, resources, acceptances, dispatch records, and lifecycle state from canonical history.
- Existing canonical histories remain openable; missing legacy thread detail is handled explicitly and never fabricated silently.
- Direct SQL mutation can no longer diverge from canonical project history.

Implementation plan:

- Extend `internal/projectstate` snapshots with applied-event metadata, resource acquisition facts, assignment epochs, project-thread facts, acceptances, and dispatches. Extend `AssignmentRunnable` with an optional typed thread snapshot for new canonical events.
- Add a store-side authoritative history collector that groups projected home-issued `project.event` records, follows only the unique child chain from the creation root, decodes each typed event, and retains the last valid head plus diagnostics.
- In `internal/store/sqlite.go`, capture legacy thread rows before projection, clear rebuildable project tables in dependency order, and reinsert projects, project events, current resources and claims, assignment history, threads, acceptances, and dispatch records from reducer snapshots. Reconcile noncanonical attempts and provenance against rebuilt authority.
- Use existing legacy thread rows only when an old runnable event lacks its now-required thread snapshot. A clean history with missing detail must stop with an actionable diagnostic rather than fabricate a launch directory or external thread.
- Refactor `CreateProject`, every mutation closure in `internal/store/projects.go`, project acceptance paths, and dispatch recording so canonical ingest/rebuild performs projection writes. Remove duplicate inserts and updates after event append.
- Add tests that empty all rebuildable project tables and recreate lifecycle, resources, assignment history, thread details, acceptances, and dispatches solely from canonical history; compare local and replica visible state; and verify legacy thread compatibility and fork/invalid-event retention.

Risks and decisions:

- Operational leases, activation/runtime/worktree sagas, and output provenance are not canonical project state. Preserve them when their rebuilt references remain valid and delete only rows whose authority disappeared.
- Rebuilding on every canonical ingest makes event application atomic with its canonical fact, but mutation code must not retain any post-ingest projection write or it will duplicate reducer output.
- Historical resource membership and claim rows are rebuilt only to the fidelity needed by current state and referenced durable facts; canonical project events remain the complete audit history.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-24 — Typed project event codec and replica reducer

Introduced a closed 18-operation project event vocabulary, typed payloads, audit-envelope decoding, and a pure reducer for lifecycle, resources, assignments, acceptances, and dispatches. Converted every local event emitter to typed data and made replica projection stop at the last valid head on forks, unknown operations, malformed data, or invalid transitions, with audit diagnostics and complete reducer/codec coverage.

### Original plan entry

## Typed project event codec and replica reducer

Create the exhaustive typed project-event vocabulary and pure reducer boundary, then make replica projection use it.

Scope:

- Define typed project-event operation constants and typed payload data for every currently emitted project event.
- Decode the existing canonical audit envelope into typed events while preserving wire compatibility.
- Validate operation-specific payloads and state transitions before applying them.
- Replace `applyReplicaProjectEvent`'s permissive string switch with the shared pure reducer.
- Stop at unknown or malformed events without advancing the replica head; surface a useful diagnostic.
- Change local project event emission helpers to accept only registered typed operations.
- Add completeness tests proving every emitted operation is registered and every registered operation has reducer coverage.

Acceptance criteria:

- Local event emitters cannot pass arbitrary operation strings.
- Replica state is produced exclusively by the typed reducer.
- Unknown or malformed events preserve the last valid state and head.
- Every supported lifecycle, resource, assignment, acceptance, and dispatch operation has a typed codec and tests.

## 2026-08-23 — Typed message purpose and mailbox addressing

Added canonical typed message purposes with deterministic legacy defaults, SQLite migration and rebuild support, RPC/client round-tripping, and purpose-aware project and protocol reply handling. Added typed sender and recipient addresses for human, agent, project, and remote presentation; moved TUI rendering to those values; kept panel kind and attribution correlated; and covered the behavior across model, event, store, bridge, node, and TUI tests.

### Original plan entry

## Typed message purpose and mailbox addressing

Make semantic distinctions that currently have to be inferred from generic `model.Message` fields explicit.

Scope:

- Introduce a typed message-purpose model that distinguishes ordinary conversation, conversational project input, structured protocol questions and answers, project output, and system notices where applicable.
- Carry purpose through canonical event payloads, SQLite projections, RPC requests, and client calls. Preserve compatibility with existing canonical events by defining deterministic defaults for records without an explicit purpose.
- Replace string-parsed sender and recipient presentation with typed mailbox addresses containing mailbox kind, mailbox ID, installation ID, and display label.
- Make all `MailboxKind` handling exhaustive. Project mailboxes must never fall through to agent or remote formatting.
- Update the Codex question/reply path so a structured answer is claimed only as a protocol response and is not also sequenced as conversational project work.
- Review `CreatePeerMessage` and other alternate envelope builders; remove them if obsolete or route them through the same typed construction boundary.
- Update TUI grouping to return one correlated presentation value containing the semantic kind and source message, rather than calculating the badge and sender from potentially different messages.

Primary areas:

- `internal/model/message.go`
- `internal/event`
- `internal/domain/store.go`
- `internal/store/sqlite.go`
- `internal/codexbridge/questions.go`
- `internal/codexbridge/replies.go`
- `internal/codexbridge/output.go`
- `internal/hqclient`
- `internal/domainrpc`
- `internal/tui/tui.go`

Risks:

- Old events do not contain the new discriminator, so fallback behavior must be stable across rebuilds and replicas.
- Structured answers already stored as project acceptances may need compatibility handling without being redelivered.

Acceptance criteria:

- Message purpose and mailbox identity are never inferred by parsing display labels or free-form `Details`.
- Conversational project replies are eligible for project sequencing; structured protocol replies are consumed only by their registered waiter.
- Human, agent, project, and remote addresses render correctly in inbox rows and panels.
- A panel badge, title, and sender are derived from the same source message.
- Existing databases and canonical histories open and rebuild without changing established message behavior.

## 2026-08-22 — Daemon-supervised durable Codex agents

Implemented daemon-owned named Codex workers with exact session history and routing, transient caller environment and working-directory inheritance, idempotent local runtime RPC, shared concurrent delivery checkpoints, detached CLI launch acknowledgements, asynchronous TUI agent/session controls, durable-name developer instructions, schema/reducer rebuild support, and control-plane/data-plane documentation. Added store, reducer, RPC/client, supervisor, bridge, CLI, TUI, node lifecycle, environment privacy, outbox isolation, retry, concurrency, and shutdown coverage.

### Original plan entry

## Named Codex agents and local session control

Make durable named agents the only identity model for HQ-managed Codex bridges, and make the local HQ daemon the owner and supervisor of every named agent runtime. `hq codex` and the TUI become thin local control-plane clients that ask the daemon to inspect, start, stop, and switch agents between their known Codex threads. Treat agent lifecycle and session assignment as installation-local control-plane state; keep mailbox messages, questions, answers, and relay delivery as the Nostr-carried data plane.

### Required behavior

- Require every `hq codex` invocation to name a durable agent with `--agent NAME`.
  - Reject bare `hq codex`.
  - Remove the anonymous bridge path and the legacy top-level `--resume THREAD_ID` interface.
  - Do not migrate or preserve existing anonymous bridge mailboxes or thread bindings. HQ is still beta; bump or reset incompatible local state as needed.
  - Do not remove generic unnamed harness mailboxes used by `hq ask`, `send`, `poll`, or non-bridge integrations unless they are independently made obsolete. This task is specifically about HQ-managed Codex bridge sessions.
- Turn `hq codex --agent NAME` into a daemon control request rather than a foreground bridge runtime.
  - Ensure the local HQ daemon is running using the same auto-start path as other HQ clients, then submit one idempotent launch request over the local control socket.
  - The daemon owns the named agent's bridge worker and the `codex app-server --stdio` child process. They survive exit of the invoking CLI or TUI and stop with the daemon.
  - `--yolo`, the optional initial prompt, the requested session action, and all other launch options travel in the local request and are applied by the daemon-owned worker.
  - Capture the invoking client's complete environment snapshot and send it transiently with the launch request. Launch the app-server with that snapshot rather than the daemon's startup environment, so credentials, `PATH`, Codex configuration, and other caller-local settings match the shell or TUI that requested the launch.
  - Capture the invoking client's current working directory and use it when `--cwd` is absent. Resolve an explicit relative `--cwd` against that caller directory before sending the request; the daemon validates the resulting absolute directory on the local machine.
  - Wait for a definitive ready or failed result before the CLI exits, print the agent name, selected thread, directory, and runtime status, and leave the worker running after success.
  - Do not fork another `hq` executable beneath the daemon. The supervisor should host the bridge worker in-process and spawn only the Codex app-server child, keeping one lifecycle authority and one local RPC surface.
- Preserve and enforce these invariants:
  - A durable agent has zero current Codex sessions before its first successful thread start, then one current selected session.
  - An offline agent retains its current selection; presence and selection are separate concepts.
  - A Codex thread is permanently bound to at most one mailbox and agent.
  - Selecting or creating another thread changes the single current selection without deleting older bindings.
  - Historical sessions remain available for later resume and cannot be reassigned to another agent.
- Give every newly created Codex thread its durable identity in developer instructions. Compose the existing structured-input instruction with language equivalent to:

  ```text
  You are operating through HQ as the durable agent named "fred".
  This name identifies your HQ mailbox across Codex thread replacements.
  Do not infer personality, permissions, authority, or repository scope from the name.

  When progress requires an answer from the human, use the structured request_user_input tool.
  ```

  Resuming a thread must retain that thread's existing instructions. Add exact protocol tests proving the name is present for new threads and that resume requests do not attempt to replace developer instructions.
- Record enough information for an agent's historical-session chooser:
  - harness and external session or thread ID;
  - the repository context and exact working directory used for that session;
  - creation or first-selection and most-recent-selection times;
  - whether it is the current selection;
  - whether the owning agent is active or offline.
  Existing `harness_bindings` and mailbox-wide contexts do not preserve an unambiguous thread-to-directory association, so introduce an explicit session projection or enrich the signed installation-private selection facts instead of inferring the directory from timestamps.
- Add domain operations and local RPC/client support to list a named agent's sessions and control its local runtime. Keep this separate from storage-only interfaces so SQLite is not responsible for spawning processes. Suggested boundaries:
  - `domain` DTOs and interfaces for session history, runtime state, and start, resume, and stop requests;
  - `domainrpc` methods and `hqclient` implementations;
  - a node-owned supervisor package that runs `codexbridge.Run`, tracks one local worker per named agent, passes the request's environment to `codex app-server`, exposes starting, running, stopping, failed, and offline state, and shuts workers down cleanly with the node.
- Runtime control must be installation-local:
  - no process command, filesystem path, ownership lease, presence, or runtime status is published through Nostr;
  - caller environment snapshots are sensitive, ephemeral control-plane inputs: never put them in canonical events, SQLite, mutation results, the bridge ledger, Nostr, logs, status details, diagnostics, or error strings, and discard them after constructing the child process environment;
  - local RPC retries may identify an environment-bearing launch by request ID and digest, but must not persist or echo the environment itself;
  - durable name and session-selection facts may remain signed installation-private events and rebuildable projections;
  - Nostr remains the data plane for mailbox traffic and relay delivery;
  - document that a future remote controller will command the owning node's control plane, and that paths are interpreted and validated on that node.
- Make runtime commands safe under retries and races:
  - use stable request IDs or idempotent desired-state handling so a lost RPC response cannot launch two bridges;
  - retain the existing named-agent lease as the final exclusion boundary;
  - all new CLI and TUI launches are daemon-owned; reject any conflicting legacy or independently owned lease clearly instead of trying to kill an unowned process;
  - select a session only after `thread/start` or exact `thread/resume` succeeds and returns the requested thread ID;
  - a failed start or resume must leave the prior durable selection unchanged;
  - switching a node-owned live agent must require confirmation, cancel the old bridge cleanly, and report if the requested replacement fails;
  - node shutdown or restart stops supervised workers and leaves their agents offline with selections intact; automatic worker restart is out of scope.
  - support concurrent workers for different named agents without bridge-ledger races or lost checkpoints; use node-owned serialization or independently persisted per-agent and per-thread ledger namespaces instead of allowing several workers to overwrite one shared sidecar file.
- Preserve mailbox routing across rotation:
  - uncorrelated root messages belong to the durable agent mailbox and may be delivered to its currently selected thread;
  - replies correlated to an older Codex thread must not leak into a replacement thread;
  - when that historical thread is selected again, its correlated pending replies become eligible;
  - an unavailable or missing historical Codex rollout produces an actionable error and does not silently select or create a different thread.
- Add an agent and session management flow to the TUI:
  - open a searchable chooser of non-retired named agents;
  - show active or offline state, current thread, and current directory;
  - after choosing an agent, show its current and historical sessions with a clear current marker, shortened thread ID, directory, and useful time metadata;
  - selecting a historical session asks the local control plane to resume that exact thread;
  - include a "new Codex thread" action with a directory input, defaulting sensibly to the agent's current directory or the TUI launch directory;
  - TUI launch and resume requests carry the TUI process's environment snapshot and launch directory under the same transient handling rules as `hq codex`;
  - resolve, clean, and verify that the path exists and is a directory on the controlled node before stopping an existing worker;
  - show starting, running, failed, ownership-conflict, and offline outcomes without freezing the Bubble Tea update loop;
  - preserve the existing inbox selection, drafts, focus, and recipient picker across agent and runtime invalidations.
- Keep CLI and TUI behavior backed by the same daemon-owned lifecycle API. Neither client may run its own bridge or define separate selection, rotation, environment, readiness, or lease semantics.
- Update `README.md`, `docs/design.md`, `docs/events.md`, embedded help, and command summaries:
  - remove anonymous `hq codex` and legacy `--resume` examples;
  - describe `hq codex` as a daemon launch client, including caller environment and working-directory inheritance, ready acknowledgement, detached lifetime, and daemon-shutdown behavior;
  - describe name injection and thread history;
  - describe the TUI controls;
  - explicitly diagram or explain the local control plane versus Nostr data plane;
  - state that no anonymous-data migration is supported.

### Likely implementation areas

- `internal/codexbridge/bridge.go`, `protocol.go`, and bridge and dispatcher tests: require a name, compose developer instructions, support exact named-session resume, and retain correlation isolation.
- `internal/domain/store.go`, `changes.go`, and new control-plane interfaces: add session-history and runtime-control models without conflating persistence and process supervision.
- `internal/event/event.go`, `validate.go`, `reducer.go`, `internal/store/sqlite.go`, and `named_agents.go`: persist and rebuild session-specific context and expose ordered history while preserving unique ownership.
- `internal/domainrpc`, `internal/hqclient`, and local-wire version tests: expose session listing and runtime commands with reconnect, idempotency, transient environment transport, and ready or failed acknowledgement behavior.
- `internal/node` plus a focused supervisor package: own all CLI- and TUI-launched Codex bridge lifecycles, construct app-server child environments, coordinate ledgers, cancellation, status, diagnostics, and node shutdown.
- `internal/tui/tui.go` and `tui_test.go`: implement the agent and session chooser, new-thread directory entry, confirmation, and asynchronous status and error handling.
- `internal/cli/app.go`, CLI and end-to-end tests, help, and documentation: require `--agent`, remove anonymous resume syntax, collect caller launch context, ensure the daemon, submit the control request, and report its result without running a foreground bridge.

### Acceptance criteria

- `hq codex` without `--agent NAME` fails before starting Codex or creating a mailbox.
- `hq codex --yolo --agent bob` auto-starts the HQ daemon when necessary, asks it to launch Bob, waits until Bob's app-server is ready, then exits successfully while Bob remains running beneath the daemon.
- The app-server receives the invoking CLI's environment exactly as the requested child environment and uses the invoking shell's current directory when `--cwd` is absent; a relative `--cwd` is resolved against that directory.
- TUI launches apply the same inheritance rules using the TUI process as the caller.
- Environment values never appear in durable storage, Nostr traffic, logs, diagnostics, status output, or RPC results, including on launch failure and retry.
- A newly created thread for `fred` receives both the durable-name and structured-human-input developer instructions.
- Creating an agent leaves it with zero selected sessions; the first successful start selects one.
- Starting a replacement preserves the previous binding and leaves exactly one current selection.
- A store rebuild returns the same current selection and complete session-specific directory history.
- Attempting to bind another agent to a known thread is rejected.
- The TUI can resume either of two historical threads for one offline agent and can start a new thread in a user-entered valid directory.
- Selection changes only after successful app-server acknowledgement; missing rollouts, invalid directories, process-start errors, and ownership conflicts are visible and non-destructive.
- Switching a live supervised agent is confirmed and cannot result in two workers for the same name.
- Old-thread replies are delivered only when their thread is selected; durable root messages follow the agent's current selection.
- TUI-launched workers continue after the TUI exits, stop with the node, and remain offline instead of auto-restarting after a node restart.
- Two different named agents can run concurrently under the daemon without ledger corruption; repeated delivery of one launch request never creates duplicate workers or app-server children.
- Control operations and runtime state never create Nostr outbox traffic.
- Store, reducer, RPC and client, supervisor, bridge, CLI, TUI, architecture, and relevant end-to-end tests pass.
# 2026-08-24 — Exhaustive project command registry

Introduced a closed, typed registry for all project commands, preserving the existing canonical JSON wire format while centralizing operation identity, codecs, creation/runtime metadata, and local home execution. Replica methods now encode typed commands, canonical ingestion rejects unknown or malformed operations deterministically without mutation, runtime handlers receive decoded typed data, and event validation derives creation semantics from the registry. Added exhaustive codec/executor completeness coverage and an integration test for unknown-command rejection.

### Original plan entry

## Exhaustive project command registry

Unify project command encoding, decoding, home execution, and runtime routing behind a typed command registry. Build this alongside or after the typed event reducer so command results emit registered event types.

Scope:

- Define typed command operations and payload codecs for every project command.
- Register each operation with validation, home-side execution, result handling, and whether runtime or supervisor participation is required.
- Replace string switches distributed across `projects.go`, `project_commands.go`, and `node.go`.
- Make unsupported operations fail explicitly without mutating project state or reporting a committed result.
- Ensure local methods and remote replica methods use the same command definitions and request validation.
- Add completeness tests proving every exported remote-capable project mutation has a codec and home handler.

Primary areas:

- `internal/domain/projects.go`
- `internal/event`
- `internal/store/projects.go`
- `internal/store/project_commands.go`
- `internal/node/node.go`
- `internal/hqclient`
- `internal/domainrpc`

Risks:

- Runtime operations have side effects and saga state; registration must not bypass mutation receipts, stale-head checks, or restart recovery.
- Command compatibility must be maintained for already queued canonical commands.

Acceptance criteria:

- Adding a command requires one typed registration rather than coordinated edits to unrelated string switches.
- Every supported operation round-trips through encode, canonical transport, decode, execute, and result projection tests.
- Unknown commands receive a deterministic rejection and cannot be silently ignored.
- Existing queued commands continue to execute after upgrade.

Implementation plan:

- Define a closed `ProjectCommandOperation` vocabulary and typed command bodies in `internal/domain`, with one registry entry per operation containing its decoder, creation/runtime metadata, and local executor where applicable.
- Make replica-facing project methods encode typed command data through the registry; make arbitrary `QueueProjectCommand` validate and normalize its operation/body before canonical transport while preserving existing JSON wire shapes.
- Replace the store's operation-string execution switch with registry decode/execute. Runtime-required entries pass typed decoded data to a typed runtime handler; unknown or malformed commands publish deterministic rejected results without mutation.
- Replace node runtime string decoding with typed command-data dispatch and preserve stable command IDs, expected heads, saga idempotency, and compatibility for already queued canonical commands.
- Route pending-creation presentation and event validation through registry metadata where layering permits, and add completeness tests covering every exported remote-capable mutation, codec round trips, local execution, runtime routing, and unknown-command rejection.

Risks and decisions:

- Existing command JSON is canonical history, so typed codecs must retain the exact current body shapes rather than introducing envelopes or renamed fields.
- Runtime commands span multiple authoritative transactions; they retain saga-owned idempotency and do not use the single local mutation receipt used by ordinary registered commands.
- The registry lives in the domain layer so store and node share operation identity and codecs without creating package cycles; concrete local execution remains expressed through a focused domain target interface.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Canonical schema-2 message contract and schema-1 legacy adapter

Added harness-neutral typed presentation, correlation, and ordered technical metadata to canonical messages; introduced strict schema-specific decoding, semantic bounds, and final signed-wire enforcement; and isolated all schema-1 structural-line compatibility inside the event projection boundary without rewriting canonical bytes. Extended message projection and SQLite persistence, migrated the primary store, harness, Codex, and TUI paths needed to exercise the contract end to end, and added compatibility, reducer, migration, behavioral, wire-bound, and race coverage.

### Original plan entry

## Canonical schema-2 message contract and schema-1 legacy adapter

Define the versioned canonical message wire contract and the single compatibility boundary that
turns historical structural detail lines into typed projections. This phase deliberately stops at
the event/model projection seam; persistence, producers, and presentation migrate in the following
phases.

Scope:

- Add shared model types for the validated presentation kinds `update`, `final-answer`, `status`,
  and `notice`; harness-neutral provider/session/operation/optional-item/optional-request
  correlation; and ordered namespaced technical sections with stable keys, optional labels, and
  string values.
- Define explicit schema constants and strict, version-specific text payload decoders. Keep the
  schema-1 text payload shape exact, add the schema-2 typed fields, make current inspection and
  reduction accept schemas 1 and 2, and leave schema-1-only inspection able to retain schema-2
  canonical bytes as unsupported.
- Add bounded validation for presentation, correlation combinations and identities, namespaces,
  keys, labels, values, section/field counts, duplicate namespace/key pairs, UTF-8, aggregate
  technical payload size, and the actual signed 64 KiB wire limit including escaped/multibyte data.
- Add a clearly named legacy schema-1 projection adapter in `internal/event`. It alone may parse
  historical `Kind`, `Phase`, harness/Codex correlation, and known project-output provenance lines.
  Scope project provenance recognition by message purpose and exact legacy shape; keep unrelated
  human details, including CLI `--details`, visible and untouched.
- Extend `event.MessageProjection` with typed presentation, correlation, and technical sections.
  Schema-2 messages project those fields directly; schema-1 messages use only the legacy adapter,
  without changing canonical bytes.

Acceptance criteria:

- Schema-2 message payloads strictly validate, sign, inspect, and project with identical typed
  semantics and ordered technical sections.
- Exact schema-1 messages still validate and project through the isolated adapter; schema-1 payloads
  reject schema-2-only fields.
- A schema-1-only reader reports a signed schema-2 message as unsupported while retaining its exact
  canonical bytes; current readers accept both versions.
- Legacy harness presentation/correlation is projected correctly without any model, store, RPC, or
  TUI parser dependency.
- A known schema-1 project-output fixture moves only recognized provenance into a legacy technical
  section, while arbitrary user details with similar words remain human-readable.
- Invalid combinations, duplicates, excessive counts/lengths, malformed UTF-8, and payloads that
  exceed the signed-wire limit after JSON escaping fail closed.

Implementation plan:

- Modify `internal/model/correlation.go` to replace line-oriented correlation parsing with the
  harness-neutral typed identity, and add `internal/model/message_semantics.go` for presentation and
  ordered technical-section DTOs plus focused validity helpers. Update model tests to cover JSON
  shape, valid combinations, and value semantics without parsing `Details`.
- Modify `internal/event/event.go` to introduce schema-1/schema-2 constants, preserve a private exact
  schema-1 payload struct, extend the public schema-2 `TextPayload`, dispatch strict payload decoding
  by content schema, default unrelated content to schema 1, and have current inspection accept both.
- Modify `internal/event/validate.go` to validate schema/type compatibility and every semantic bound,
  then make signing enforce `MaxWireBytes` on the final serialized event rather than relying on
  component limits. Add table-driven failing tests first for unknown fields, invalid correlation,
  duplicate technical fields, escaped/multibyte overflow, and older-reader retention.
- Add `internal/event/legacy_message.go` with a pure `projectLegacyMessage` adapter. It will split only
  exact historical structural lines, preserve human line order/content, recognize project provenance
  only for `project-output`/`system-notice` shapes, and emit stable `hq.legacy.*` sections.
- Modify `internal/event/reducer.go` so schema-2 projects directly and schema-1 delegates to the
  adapter. Add reducer fixtures for harness correlation, Codex aliases, project provenance, ordinary
  lookalike user details, shuffled arrival, duplicate delivery, and unchanged raw bytes.

Tests to add first:

- Event validation tests for every presentation kind, all legal correlation shapes, partial/invalid
  identities, technical ordering, duplicate namespace/key pairs, invalid UTF-8, field/count/aggregate
  bounds, and worst-case JSON escaping under the wire limit.
- Compatibility tests proving strict schema-1 rejection of schema-2 fields, current schema-1/schema-2
  acceptance, schema-1-only unsupported retention, and canonical byte preservation.
- Reducer tests proving direct schema-2 projection and isolated schema-1 conversion, including known
  project output versus user-authored lookalikes.

Risks and decisions:

- Go strings are always byte sequences, so validation must check UTF-8 before byte-length bounds and
  wire tests must measure the fully signed JSON envelope.
- Empty correlation is valid; once any correlation member is present, provider and session are
  required, while operation/item/request remain opaque optional identifiers subject only to bounds.
- Technical sections preserve producer order for display, but duplicate namespace/key pairs are
  rejected globally so consumers never need precedence rules.
- `Details` stays byte-for-byte human content for schema 2. The schema-1 adapter may remove only exact
  recognized legacy structural lines from the projected copy; it never rewrites canonical history.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Typed message projection persistence and RPC round-trip

Completed and proved the typed message round trip across local create/filter/reply, project-input routing, SQLite restart and forced canonical rebuild, schema-1 rebuild compatibility, encrypted peer replication and duplicate delivery, domain RPC, and the HQ client. Empty-correlation replies now inherit the original full correlation while explicit reply correlation remains authoritative; older message JSON without typed fields remains compatible.

### Original plan entry

## Typed message projection persistence and RPC round-trip

Finish and prove the typed message read/write path from local domain clients through canonical
events and disposable SQLite projections. The schema-2 DTOs, projection columns, and primary
writers already exist; this phase makes their round-trip guarantees explicit and closes gaps around
reply inheritance, transport, rebuilds, and RPC compatibility.

Scope:

- Add reusable test fixtures and equality assertions for presentation, full provider/session/
  operation/item/request correlation, ordered technical sections, human `Details`, purpose, and
  context.
- Prove ordinary create/get/list and explicit correlation filters preserve typed values without
  consulting `Details`. Keep the flat harness columns as indexed/backward-compatible read fields,
  but make the typed `Correlation` value authoritative for new writes.
- Make store replies inherit the original typed correlation when the caller leaves correlation
  empty, while preserving an explicitly supplied valid correlation unchanged. Ensure repeated
  payload reconstruction for project routing retains every typed field.
- Prove schema-2 peer transport and duplicate delivery preserve identical typed values and ordered
  technical sections on the receiving installation.
- Prove close/reopen and a forced canonical projection rebuild restore the same typed message value
  from signed bytes, with no dependence on the old projection columns.
- Add domain RPC and HQ client round-trip tests for create, reply, get, list, and conversation history
  responses. Verify strict request decoding still rejects unknown fields and older JSON that omits
  the new fields remains compatible.
- Add an explicit schema-1 canonical fixture to the store rebuild tests, proving only the event-layer
  legacy adapter supplies typed correlation/presentation and that no store parser is involved.

Acceptance criteria:

- Local create, get/list, reply, restart, forced rebuild, peer replication, duplicate delivery,
  domain RPC, and HQ client paths preserve identical presentation, full correlation, technical
  section order/labels/values, human details, purpose, and context.
- An empty-correlation reply inherits all original correlation members; an explicit reply
  correlation is not overwritten.
- Correlation filters continue using indexed provider/session/operation projection columns and work
  when `Details` has no structural lines.
- Schema-1 rebuild compatibility continues to originate only in `internal/event`; arbitrary
  schema-2 human details that resemble legacy lines remain unchanged.
- Existing message JSON without typed fields still decodes, and strict RPC request envelopes still
  reject unknown fields.

Implementation plan:

- Add failing store tests first in `internal/store/sqlite_test.go` for typed create/reply/restart/
  forced-rebuild equality and schema-1 rebuild compatibility, plus transport coverage in
  `internal/store/transport_test.go` for typed replication and duplicate delivery.
- Update `internal/store/sqlite.go` only where those tests expose gaps: centralize canonical typed
  equality helpers in tests, inherit original correlation for empty replies, and ensure every
  project-routing payload reconstruction uses the shared schema-2 payload builder.
- Add focused `internal/domainrpc` service tests that capture typed create/reply requests and return
  typed get/list/history results through the actual local-wire JSON boundary.
- Add focused `internal/hqclient` tests using a real in-memory local-wire server to compare typed
  requests and responses rather than merely checking method names.
- Retain the current flat harness fields and filter DTOs as compatibility/index surfaces for now;
  producer cleanup and any eventual API removal remain in the following phase.

Risks and decisions:

- SQLite JSON decoding commonly turns a nil technical-section slice into an empty slice. Equality
  assertions normalize only this representational difference and remain strict about section and
  field order.
- Canonical timestamps are second-granularity, so fixtures compare semantic message fields rather
  than caller-side subsecond timestamps.
- Peer transport tests must compare the canonical typed projection after unwrap, not encrypted
  wrapper bytes whose recipient-specific envelope is intentionally different.
- Reply inheritance applies only when the caller supplies an empty correlation; explicit values are
  validated at the schema-2 event boundary and remain authoritative.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Typed message producers and behavioral consumers

Migrated the final project message writers to schema 2: project outputs now preserve caller semantics and append ordered typed provenance, while resource-health and pending-work notices use typed notice presentation and project namespaces. Project-output retries reconcile through the authoritative provenance row with strict typed collision checks, the TUI no longer duplicates flat correlation or retains an unused details parser, and architecture coverage guards against new structural Details literals.

### Original plan entry

## Typed message producers and behavioral consumers

Finish the writer migration after an inventory confirmed the remaining schema-1 structural message
authors are confined to project output provenance, project resource-health notices, and closed/
archived project pending-work notices. Generic harness, Codex compatibility, ordinary store/peer,
reply, retry collision checks, and primary TUI compose paths already use typed semantics.

Scope:

- Convert `CreateProjectOutput` to schema 2. Preserve caller presentation, full correlation,
  technical sections, human details, purpose, and context; append ordered diagnostic provenance in
  `hq.project.output_provenance` without copying it into `Details`.
- Preserve the existing authoritative `project_output_provenance` table and actor-label behavior.
  Emit project, assignment, and project-thread IDs for every output; append late/current-assignment/
  current-agent/current-thread diagnostics in stable order only for late output.
- Convert resource-health notices to schema 2 with `notice` presentation and an ordered
  `hq.project.resource_health` technical section. Keep the body human-readable and move project,
  resource, previous/current health, and optional health JSON out of `Details`.
- Convert closed/archived project pending-work notices to schema 2 with `notice` presentation and an
  ordered `hq.project.pending_message` section. Keep project behavior sourced from typed project
  state and acceptance records, never from technical fields.
- Stop TUI compose code from duplicating typed correlation into deprecated flat harness fields and
  delete the now-unused generic `detailValue` parser. Retain read-side flat fields only as the
  compatibility/index surface established in the persistence phase.
- Keep all idempotency/collision checks strict over body, human details, presentation, correlation,
  and ordered technical sections. Add a source-level conformance test preventing new non-legacy
  structural message writers outside `internal/event/legacy_message.go`.

Acceptance criteria:

- Every non-legacy message writer emits schema 2; project writers contain no `Kind`, harness, or
  project-provenance protocol lines in `Details`.
- Project output retains caller typed semantics and existing technical sections, adds stable
  `hq.project.output_provenance`, preserves persistent provenance rows, and marks late output exactly
  as before.
- Project resource-health and pending-work notices present as typed notices with generic technical
  sections and unchanged human-readable bodies.
- Routing, acceptance, late-output classification, actor labels, and project lifecycle behavior are
  invariant when technical keys, labels, or values are not consulted.
- TUI-created messages author only `Correlation`; no normal-path consumer calls a details parser.

Implementation plan:

- Update project tests first to require schema 2, typed presentation/correlation preservation,
  unchanged human details, stable new namespaces/field order, existing-section retention, and the
  unchanged `project_output_provenance` row.
- Refactor `internal/store/project_delivery.go` to build a copied message with appended provenance,
  marshal through `textPayloadForMessage`, and set `MessageSchemaVersion` explicitly.
- Refactor `internal/store/projects.go` and `internal/store/project_inbound.go` notice payloads to
  typed `TextPayload` values with presentation and technical sections, preserving account audience,
  membership parents, body, actor label, timestamps, and project event ordering.
- Remove the TUI's redundant flat-field writes and unused `detailValue` helper; migrate any tests
  that still construct normal-path semantics as structural details.
- Add or extend architecture/conformance coverage that inventories `event.TextPayload` message
  writers and rejects structural `Details` prefixes outside the isolated legacy adapter and
  intentional compatibility fixtures.

Risks and decisions:

- Technical metadata is diagnostic only. The provenance table and typed project state remain the
  source of truth for delivery, assignment, and late-output behavior.
- Appending producer metadata must copy the section slice before mutation so caller-owned values and
  idempotency expectations remain stable.
- Human `Details` are no longer trimmed or augmented by project delivery; schema-2 preserves them as
  supplied, including blank lines and legacy-looking prose.
- Project notice bodies remain visible when technical details are collapsed; identifiers and health
  JSON are disclosed only through the generic technical panel.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Generic technical presentation, documentation, and conformance

Removed the TUI's remaining structural-`Details` parser so human details always render unchanged and all presentation, correlation disclosure, and thread-name annotation use typed message fields. Arbitrary ordered technical namespaces now remain wholly behind `i`, with conformance tests proving label/key order, unknown-namespace rendering, structural-lookalike human text preservation, and behavioral invariance. Documented the schema-2 contract, validation bounds, legacy reducer boundary, harness and project namespaces, SQLite schema 30 round trip, and generic TUI disclosure across the event, harness, project, design, and README surfaces; full tests, vet, compatibility suites, diff checks, and store/bridge/TUI race tests pass.

### Original plan entry

## Generic technical presentation, documentation, and conformance

Finish the schema-2 message work at the presentation and documentation boundary. `Details` is
always human content and must be rendered without parsing or rewriting; presentation, correlation,
technical disclosure, and thread-name annotation must come only from typed message fields. Keep
technical metadata behaviorally inert and render every namespace generically under `i`.

### Implementation plan

1. Make the TUI presentation path entirely typed in `internal/tui/tui.go`.
   - Delete `presentationDetails` and its hard-coded `Kind`, `Phase`, harness, and HQ prefix
     allowlist.
   - Render non-empty `Message.Details` literally in both collapsed and expanded views.
   - Convert `technicalIdentifiers` into an app-aware typed renderer so provider/session
     correlation can resolve a mutable thread name through `threadSessions` while retaining the
     immutable session ID.
   - Preserve the existing derived `hq.message.identifiers` and `hq.message.correlation` groups,
     field order, label-or-key display fallback, arbitrary namespace rendering, and whole-section
     `i` disclosure. The collapsed hint must depend only on typed/derived technical content and
     technical context, never on text inside `Details`.

2. Add presentation and behavior conformance coverage in `internal/tui/tui_test.go`, preferably as
   failing tests before the production edit.
   - Replace the legacy expanded-details annotation test with a typed-correlation test that proves
     the provider/session pair resolves the friendly thread name only in the expanded technical
     block.
   - Prove structural-looking human lines such as `Kind:`, `Harness session:`, and project-like
     labels stay visible unchanged before and after `i`.
   - Prove an unknown namespace renders generically only after `i`, preserves section/field order,
     uses labels only for display, and leaves ordinary details visible.
   - Prove changing technical namespaces, keys, and labels cannot change conversation grouping,
     action-unit grouping, final-answer selection, or reply targeting when typed presentation and
     correlation are unchanged.
   - Retain the existing collapsed-border hint, identifier disclosure, Markdown/body, and human
     details assertions.

3. Strengthen the source-level contract in `internal/architecture/dependencies_test.go`.
   - Add a guard that production TUI code does not contain the historical structural-details
     protocol prefixes, while leaving the isolated `internal/event/legacy_message.go` schema-1
     adapter as the only compatibility parser.
   - Keep the existing producer guard that prevents new text-payload literals from embedding
     structural prefixes in `Details`.

4. Document the complete contract without treating diagnostic metadata as semantics.
   - Update `docs/events.md` with per-event schema support, the strict schema-1/schema-2 text
     payload shapes, typed presentation/correlation fields, technical-section bounds and namespace
     conventions, exact-byte unsupported retention, the isolated legacy projection rule, and the
     64 KiB signed-wire limit.
   - Update `docs/harnesses.md` with typed producer namespaces (`hq.harness.output`,
     `hq.harness.status`, and `hq.harness.request`), opaque provider correlation, reply copying, and
     the rule that human instructions/errors/options remain in `Details`.
   - Update `docs/projects.md` with typed project message semantics, diagnostic provenance and
     notice namespaces, preserved project-output provenance/idempotency, and the prohibition on
     reading technical sections for project behavior.
   - Update `docs/design.md` with SQLite schema 30, typed projection/RPC round trips, schema-1
     compatibility at the reducer boundary, and the semantic-versus-diagnostic architecture rule.
   - Update `README.md` TUI/help text so `i` is described as generic technical disclosure, human
     details remain visible, unknown namespaces need no UI allowlist, and friendly thread names are
     derived from typed provider/session identity.

5. Verify cross-layer conformance and close only regressions caused by this phase.
   - Run focused TUI and architecture tests first.
   - Run `go test ./...`, `go vet ./...`, `git diff --check`, and race tests for
     `./internal/store`, `./internal/harnessbridge`, `./internal/codexbridge`, and `./internal/tui`.
   - Re-run canonical event/store/RPC/client/transport tests that exercise schema-2 validation,
     signing, projection, persistence, rebuild, replication, unsupported schema retention,
     schema-1 compatibility, malformed/duplicate/escaped/multibyte/wire-bound rejection, and typed
     JSON round trips. No canonical history or schema-1 payload is rewritten by this phase.

### Risks and decisions

- `Details` can legitimately begin with historical protocol-looking words. The new contract favors
  preserving that human text; only schema-1 projection may have already separated recognized
  producer-shaped legacy lines before they reach the TUI.
- Thread names are mutable display metadata. They must never be copied into immutable messages or
  used as correlation identity; the typed provider/session pair remains authoritative.
- Built-in identifiers and repository/source context are derived technical display groups rather
  than serialized technical sections. They remain inert and hidden with the same `i` control.
- No schema or producer changes are planned here: those paths were implemented and tested in the
  preceding stacked phases. Any missing cross-layer behavior found during verification will be
  fixed in the owning package and documented before completion.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Typed canonical message semantics final integration audit

Audited the complete typed-message umbrella against the four preceding stacked phases and mapped every schema, projection, persistence, RPC, replication, legacy, producer, presentation, validation, wire-bound, and behavior requirement to passing coverage. Closed the one residual gap by removing TUI fallbacks from typed `Correlation` to deprecated flat harness fields: replies now copy the typed object directly, conversation keys use typed provider/session identity, and action units use typed operation identity. Added regression coverage proving flat-only fields cannot merge conversations, choose an action, or leak into replies, while conflicting flat fields cannot override valid typed correlation. Full tests, fresh compatibility suites, vet, diff checks, and store/harness/Codex/TUI race suites pass.

### Original plan entry

## Typed canonical message semantics and technical metadata — final integration audit

Message `Details` currently mixes human-readable supplementary content with machine-readable fields. Canonical reduction parses harness correlation back out of line-oriented text; the TUI separately parses presentation kind, correlation, request identity, and visibility using hard-coded key prefixes. Project delivery adds another set of raw keys. This makes display labels an implicit cross-module protocol.

Make message structure explicit: behaviorally meaningful data travels through typed canonical fields, while diagnostic/display-only key/value data travels through namespaced technical sections. Message body and human details must never be parsed to recover structure or drive behavior.

Implement the following:

- Introduce canonical message semantics shared through the event, projection, model, store, RPC, client, and TUI paths:
  - a validated presentation-kind enum for `update`, `final-answer`, `status`, and `notice`;
  - harness-neutral correlation containing provider, session, operation, optional item, and optional request IDs;
  - namespaced technical sections containing stable machine keys, optional display labels, and string values.
- Treat the technical-section container as always technical. The TUI must hide or show entire sections with `i` without inspecting namespaces, keys, labels, or values. Namespaces identify provenance; keys identify fields; labels are presentation only. Do not add downstream checks such as `key == "Project"` or producer-specific prefix allowlists.
- Reserve typed semantic fields for anything used by routing, conversation identity, grouping, ordering, reply/archive selection, request correlation, final-answer selection, authorization, or other behavior. Generic technical metadata must remain inert: no code may read it to make domain decisions. Promote any value that later becomes behavioral into a dedicated typed field.
- Define bounded validation for presentation kinds, correlation identities, namespaces, keys, labels, values, section and field counts, UTF-8, duplicate namespace/key pairs, and aggregate payload/wire size. Keep provider values opaque and harness-neutral; do not introduce Codex protocol terms into canonical or domain types. Technical sections are display disclosure, not an access-control or secret-storage mechanism.
- Add explicit schema-version support for the extended text payload:
  - retain an exact schema-1 decoder;
  - add a schema-2 text payload carrying typed semantics and technical sections;
  - make new message writers explicitly emit schema 2 while unrelated event types may remain schema 1;
  - have current readers accept schemas 1 and 2 with strict, version-specific payload decoding;
  - ensure older binaries retain schema-2 bytes as unsupported events rather than treating an added schema-1 field as invalid;
  - do not rewrite canonical history.
- Centralize schema-1 compatibility parsing at the canonical projection boundary in a clearly named legacy adapter. It may decode the historical `Kind`, `Phase`, Codex/harness correlation, and project-provenance lines into typed projections and legacy technical sections. No store query, RPC client, TUI code, or new writer may call that parser. Scope legacy project decoding by known message purpose and shape so ordinary user-supplied details are not casually reclassified.
- Extend `event.MessageProjection`, `model.Message`, and the SQLite message projection with typed presentation/correlation fields and technical-section JSON. During full rebuild:
  - schema-2 messages project directly from typed payload fields;
  - schema-1 messages use only the legacy adapter;
  - raw canonical bytes remain untouched;
  - recognized legacy technical lines are presented through technical sections rather than leaking into always-visible human details.
- Remove normal-path uses of `model.ParseMessageCorrelation`, `detailValue`, presentation-kind text parsing, and the TUI technical-prefix list. Delete or confine those helpers to the schema-1 adapter after all consumers use projected typed data.
- Update every message producer, including generic harness output/status/questions/notices, Codex compatibility paths, project output provenance, project system notices, TUI-created replies and session-targeted messages, peer/account message creation, and retry/reconciliation comparisons:
  - populate typed semantics directly;
  - put only human-readable instructions, errors, choices, schemas, and explanations in `Details`;
  - emit diagnostic attributes in stable namespaces such as `hq.harness.output`, `hq.harness.request`, and `hq.project.output_provenance`;
  - do not duplicate message ID in metadata when the canonical/model message ID already supplies it;
  - copy typed harness correlation onto replies instead of serializing and reparsing it.
- Preserve existing project-output provenance persistence. Its technical section represents display/diagnostic provenance, while any field needed for project behavior remains in typed project state or is promoted to a dedicated typed message field rather than read from metadata.
- Update output idempotency and collision checks so typed semantics and technical sections participate where relevant. Ensure repeated payload construction in store routing paths cannot accidentally drop the new fields.
- Persist and scan the new projection columns/JSON through SQLite migration and rebuild, and verify domain RPC and client serialization round-trip them without converting them back to text.
- Render expanded technical sections generically with their namespace visible, preserving field order and using labels only for display. Continue showing built-in message/event/thread/installation identifiers under `i`, grouped under explicit derived HQ namespaces. Thread-name annotation must use typed provider/session identity.
- Keep schema-1 human details readable, preserve CLI/user-supplied `--details` as human content, and document that `Details` is not a structural channel.
- Update `docs/events.md`, `docs/harnesses.md`, `docs/projects.md`, `docs/design.md`, and relevant README TUI/help text with the schema-2 message contract, namespace conventions, typed-versus-technical boundary, legacy compatibility rule, and `i` behavior.

Expected implementation areas include:

- `internal/event/{event.go,validate.go,reducer.go}` and version/compatibility tests;
- `internal/model/{message.go,correlation.go}` or replacement semantic types;
- `internal/store/{sqlite.go,transport.go,project_delivery.go,projects.go,project_inbound.go}` plus migration/rebuild tests;
- `internal/harnessbridge/{events.go,questions.go,bridge.go}` and remaining Codex adapter compatibility paths;
- `internal/domainrpc`, `internal/hqclient`, and JSON compatibility tests;
- `internal/tui/{tui.go,markdown.go}` and presentation/reply/grouping tests;
- canonical event, harness, project, design, and TUI documentation.

Completion requires tests proving:

- schema-2 messages validate, sign, project, persist, RPC-round-trip, replicate, and rebuild with identical typed semantics and technical sections;
- schema-1 messages still project with correct correlation and presentation through the isolated legacy adapter;
- a schema-1 project-output fixture hides legacy project provenance until `i`, while arbitrary user details using similar words remain visible;
- an older schema-1-only reader classifies schema-2 messages as unsupported and retains their canonical bytes;
- shuffled arrival, duplicate delivery, restart, and full rebuild produce equivalent typed message projections;
- provider/session collisions remain isolated without consulting `Details`;
- final-answer selection, conversation grouping, action-unit grouping, request reply targeting, and reply correlation work when `Details` contains no structural lines;
- changing a technical key or label cannot change behavior, and unknown namespaces render generically only when technical details are expanded;
- project and harness producers no longer serialize structural correlation or presentation kind into `Details`;
- malformed, duplicate, or oversized technical sections and invalid correlation combinations fail validation, including worst-case escaped and multibyte payloads under the signed-wire limit;
- existing human details, approvals, validation errors, options, and schemas remain visible and usable;
- `go test ./...`, relevant store/TUI/harness race tests, `go vet ./...`, and `git diff --check` pass.

### Final integration audit execution plan

The four preceding stacked phases implemented the schema/model/legacy boundary, projection and RPC
round trip, producer migration, and generic presentation/documentation work. This final pass will
not duplicate those changes. It will reconcile every completion criterion above against the
current tree and close the one residual normal-path fallback found during source inspection.

1. Make typed correlation authoritative for TUI behavior in `internal/tui/tui.go`.
   - Remove `correlationForMessage` and its fallback from empty `Message.Correlation` to deprecated
     flat `HarnessProvider`, `HarnessSessionID`, and `HarnessOperationID` projection fields.
   - Have reply creation copy `answerQ.Correlation` directly, conversation grouping read
     `Message.Correlation` directly, and action-unit grouping use only
     `Message.Correlation.OperationID` before the ordinary causal thread/message fallback.
   - Keep the flat SQLite columns and model JSON fields for indexed queries and older local-wire
     compatibility; current store scans already hydrate `Correlation`, so presentation code does
     not need a second authority.

2. Add regression coverage in `internal/tui/tui_test.go`.
   - Prove flat-only harness fields cannot merge distinct causal conversations, choose a harness
     action unit, or leak into a TUI-created reply.
   - Prove messages with identical typed provider/session/operation correlation still group and
     target replies identically even when their deprecated flat fields conflict.
   - Retain the existing provider-collision, final-answer, request-target, technical-invariance,
     structural-lookalike, and unknown-namespace tests as the behavioral contract.

3. Audit the full umbrella acceptance matrix without speculative rewrites.
   - Confirm production `TechnicalSections` reads are limited to validation, persistence,
     equality/idempotency comparison, and generic rendering; producer-specific project behavior
     must remain in typed project state.
   - Confirm all structural text parsing is isolated in `internal/event/legacy_message.go`, all
     current text-message writers select schema 2, and store/RPC/client/project/peer paths preserve
     typed fields.
   - Map schema/sign/project/rebuild/replication/unsupported-reader, schema-1 project-shape,
     provider collision, producer, human-details, malformed/bounds, escaped/multibyte wire, and UI
     disclosure requirements to focused tests. Add a missing test only if the behavior is not
     already proved compositionally.

4. Verify and commit the audit closure.
   - Run focused TUI and architecture tests plus fresh event/store/domain-RPC/client/transport
     compatibility suites.
   - Run `go test ./...`, `go vet ./...`, `git diff --check`, and race tests for
     `./internal/store`, `./internal/harnessbridge`, `./internal/codexbridge`, and `./internal/tui`.
   - Commit with Conventional Commits, remove this entire umbrella entry from `PLAN.md`, and append
     the actual audit summary plus this complete pre-work entry verbatim to `COMPLETED.md`.

### Risks and decisions

- Deprecated flat message fields remain serialized for compatibility, but they are derived/indexed
  projections rather than an independent semantic source. Removing their TUI fallback may change
  hand-constructed legacy JSON that omits `Correlation`; supported store/RPC reads populate the
  typed object, and the old client decoder already treats absent typed fields as non-semantic.
- Equality checks may inspect technical sections to reject a deterministic-ID collision. That is
  content integrity, not a domain decision derived from a particular namespace/key/value, and must
  continue comparing the complete ordered value.
- The schema-1 adapter intentionally parses historical details. No new parser or canonical rewrite
  is permitted during this audit.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

# 2026-08-25 — Canonical harness activity event and deterministic reducer

Added the schema-2 `harness.activity` event with harness-neutral typed correlation, activity kind/status/content, occurrence time, runtime lifetime, and source sequence under explicit UTF-8, identity, shape, payload, and signed-wire bounds. Activity is restricted to installation-private or active-account audiences from a non-human source mailbox; peer/public/recipient forms and revoked or unrelated account sources fail closed, while schema-1-only readers retain authentic local and account-addressed bytes as unsupported. The pure reducer now produces full-mailbox/provider-isolated logical activity projections and a stable causal message/activity conversation order, coalescing repeated snapshot/item keys without altering any message or thread state. All kinds, strict decoding, malformed shapes, escaped/multibyte bounds, authorization, unsupported compatibility, collisions, duplicates, shuffled arrival, causal order, and message invariance are covered; full tests, vet, diff checks, and event/domain races pass.

### Original plan entry

## Canonical harness activity event and deterministic reducer

Define the signed harness-neutral activity contract and its pure canonical projection before any
store writer begins emitting it. This phase establishes validation, authorization, compatibility,
stable identity, and order-independent reduction while leaving the current local SQLite activity
writer and read API operational until the following phase.

### Implementation plan

1. Extend the shared activity model in `internal/domain/harness_activity.go` and canonical schema in
   `internal/event/event.go`.
   - Add canonical event ID, originating full mailbox address/installation identity, runtime
     lifetime ID, provider event sequence, and stable canonical display-order information to the
     projected activity without turning it into a message or inbox action.
   - Add `TypeHarnessActivity` under schema 2 and a strict `HarnessActivityPayload` containing the
     existing harness-neutral kind/status/title/body/truncation/occurrence fields plus the shared
     typed provider/session/operation/item correlation, runtime lifetime, and source sequence.
   - Keep provider/session/operation/item values opaque. Do not add Codex methods, JSON-RPC data,
     raw provider payloads, request correlation, or technical message sections.
   - Define explicit UTF-8 and byte/count bounds that leave enough envelope space below the actual
     64 KiB signed-wire limit; keep the final wire-size check authoritative.

2. Validate scope, shape, and kind-specific semantics in `internal/event/validate.go`.
   - Accept activity only as schema 2, installation-private or account-addressed, with exactly one
     originating sender mailbox and no recipient/thread ID. Reject peer-addressed and public forms.
   - Require provider/session/operation identity, runtime lifetime, positive source sequence, and a
     valid occurrence time. Require an item for command/file/tool/progress and forbid it for
     operation/plan/diff snapshots.
   - Preserve existing status/body/title rules: operation needs status; plan/diff/progress need
     bodies; command/file/tool need titles and terminal status. Validate printable identities,
     UTF-8, timestamps, truncation state, and exact payload/wire bounds.
   - Extend account authorization so an active account device may publish activity only from its
     own non-human mailbox into the named account audience. Local-root installation-private
     activity remains local; revoked/unrelated, peer, recipient-addressed, and public activity fail
     closed.

3. Add deterministic activity projection in `internal/event/reducer.go`.
   - Add a `HarnessActivityProjection` and `State.HarnessActivities` keyed by the full originating
     mailbox plus provider/session/operation/kind/item logical key, preventing provider or mailbox
     collisions.
   - Project only authorized, causally usable events and retain the winning canonical event ID,
     exact sender, account audience, typed correlation, source lifetime/sequence, signed occurrence
     time, and canonical display position.
   - Apply activities in canonical display order so operation/plan/diff snapshots and repeated item
     keys are deterministic latest-wins projections independent of receipt order or duplicate
     delivery. Completed command/file/tool keys and distinct progress items remain separate rows;
     canonical records remain retained even when a logical projection is superseded.
   - Do not change message/thread projection, inbox state, final-answer selection, or legacy message
     behavior.

4. Add focused contract and reducer tests in `internal/event/event_test.go` and
   `internal/event/reducer_test.go`.
   - Cover every activity kind through validate/sign/inspect/project and assert exact typed values,
     full mailbox association, event ID, source sequence, and deterministic order.
   - Cover missing/invalid identities, kind/status/title/body/item combinations, UTF-8, oversized
     title/body/identities, escaped and multibyte signed-wire boundaries, invalid occurrence time,
     zero sequence, and strict unknown fields.
   - Prove shuffled arrival and duplicates yield byte-for-byte equivalent projections; repeated
     logical keys coalesce while distinct provider/session/mailbox/item identities stay isolated.
   - Prove schema-1-only readers retain schema-2 activity bytes as unsupported and prove local,
     active-account, revoked/unrelated-account, peer, recipient, and public authorization outcomes.
   - Assert activity cannot alter message or thread projections.

### Risks and decisions

- Source sequence is provider-runtime evidence, not a global clock. Canonical causal/display order
  chooses projection winners; lifetime and sequence are retained and validated for deterministic
  same-source ordering and diagnostics, not compared across unrelated runtimes.
- The current unsigned `harness_activities` table remains untouched in this phase. The next phase
  will migrate it to a disposable canonical projection and intentionally discard legacy unsigned
  rows rather than manufacture signed history.
- Dynamic truncation belongs at the canonical authoring boundary in the store/bridge phase. This
  phase defines safe maxima and rejection behavior so callers cannot bypass final wire validation.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-25 — Canonical activity SQLite authoring, privacy, and rebuild

Canonical harness activity authoring now validates typed correlation plus runtime/sequence identity, dynamically fits escaped UTF-8 against the signed 64 KiB envelope, signs account-addressed schema-2 events, and uses ordinary canonical ingest and fanout. Schema 31 discards unsigned legacy rows and rebuilds a source-complete, canonically ordered projection with deterministic 200-row progress retention; same-time reducer ordering honors occurrence and provider sequence. The legacy read API now exposes event, source, audience, correlation, runtime, sequence, and display metadata, while the public write RPC/client API has been removed. Exact replay, migration, restart/rebuild, retention, message invariance, account gift-wrap convergence, full-suite, vet, and race coverage pass.

### Original plan entry

## Canonical activity SQLite authoring, privacy, and rebuild

Replace the unsigned local activity mutation with a store operation that validates, dynamically
fits, signs, authorizes, appends, fans out, and projects canonical activity. Migrate the disposable
SQLite table to event/source/order columns, discard legacy unsigned rows, rebuild solely from the
canonical log, implement deterministic snapshot/progress retention, and prove restart, rebuild,
duplicate, shuffled, cross-device, revoked/unrelated/peer/public, and 64 KiB behavior. Keep the
legacy read API temporarily while removing or narrowing public write RPC access.

Scope:

- Make `UpsertHarnessActivity` an internal producer operation that converts the domain value to a
  schema-2 `harness.activity` content, signs it with the local installation identity, and appends it
  through the ordinary canonical ingest transaction. Replays with identical source identity,
  sequence, occurrence time, and content must produce the same event ID.
- Require full typed correlation plus runtime-lifetime identity and provider sequence at the
  canonical authoring boundary. Preserve the flat harness/session/operation/item fields only as a
  temporary read-side compatibility view. Give the existing bridge the minimum metadata plumbing
  needed to satisfy that boundary; bounded lossless buffering remains in the following task.
- Address activity to the active local human account and use its membership parents so normal
  outbox fanout synchronizes it to active devices. Allow installation-private content only through
  canonical ingest for genuinely local-only callers; the producer operation must never create
  peer-addressed or public activity.
- Dynamically fit title/body text by UTF-8 boundaries against the actual signed 64 KiB envelope,
  retaining kind-specific presentation limits and setting `truncated` whenever fitting changes the
  input. Fail if required metadata alone cannot fit or validate.
- Replace the legacy `harness_activities` table with a disposable canonical projection containing
  event ID, full source installation/mailbox identity, typed correlation columns, runtime/sequence,
  occurrence time, and canonical display order. The logical-key uniqueness must include source
  installation and mailbox as well as provider/session/operation/kind/item.
- Schema migration must drop legacy unsigned activity rows, create the canonical projection shape,
  and invalidate the projection checkpoint. Full projection rebuild must clear the table and insert
  only reduced canonical activities in reducer order.
- Retain only the canonical latest-wins snapshot per reducer logical key and, after projection,
  deterministically retain the newest 200 progress rows per full source/provider session using
  canonical display order rather than receipt time. Keep the query cap at 1,000 chronological rows.
- Extend the temporary read filter with optional source installation identity, populate canonical
  event/source/correlation/runtime/sequence/display fields, and derive legacy flat fields from the
  typed correlation rather than storing competing semantics.
- Include activity invalidations in canonical append/inbound paths without changing message,
  inbox, unread, archive, reply, draft, delivery, or project behavior.
- Remove `activity/upsert` from the public domain RPC protocol and HQ client while retaining the
  read RPC. The in-process bridge/store writer contract remains narrow and daemon-internal.

Implementation plan:

- Add a schema migration and base schema for the canonical activity projection, including source-
  complete uniqueness and query/progress indexes; explicitly discard the version-30 unsigned rows.
- Rework store authoring normalization to accept typed source metadata, choose account audience and
  membership parents, sign/fits-check iteratively at UTF-8 boundaries, and append through canonical
  ingest so authorization, fanout, reduction, mutation receipts, and invalidations share one path.
- Add runtime identity and provider sequence to bridge-produced activity values without changing
  the existing queue/drop policy in this phase.
- Project `event.State.HarnessActivities` during every rebuild, insert rows by
  `HarnessActivityOrder`, prune progress deterministically, and update the legacy list query to
  return canonical fields and order.
- Remove the public write method, request type, server dispatch, client method, and compatibility
  expectations, leaving list compatibility intact.
- Replace unsigned-table tests with canonical authoring, exact replay/duplicate, restart/rebuild,
  shuffled ingest, source isolation, account fanout/privacy, migration-discard, retention/order, and
  escaped/multibyte signed-wire tests. Assert byte-equivalent projections and message-state
  invariance where appropriate.

Risks and decisions:

- The reducer, not SQLite conflict timing, remains authoritative for coalesced winners. SQLite only
  materializes its selected entries and applies the documented bounded progress projection.
- Canonical events remain retained even after progress rows are pruned from the disposable table;
  a rebuild must deterministically reproduce the same bounded table.
- Signing time is the normalized occurrence time so an identical producer replay is idempotent.
  The bridge's strictly increasing provider sequence and per-runtime identity distinguish distinct
  values that otherwise share a logical key.
- Account membership parents are resolved at authoring time. Revoked and unrelated senders remain
  rejected by canonical authorization, and peer/public attempts are covered at the ingest boundary.
- The next phase owns queue backpressure, coalescing, shutdown draining, and serialized relative
  output/activity ordering; this phase must not silently redesign those behaviors.

Acceptance criteria:

- Every bridge-supported kind authors a projected schema-2 event whose event ID and full source
  metadata survive restart and complete projection rebuild.
- Identical replay and duplicate ingest are idempotent; shuffled canonical arrival produces the
  same rows and display ordering; provider and source-mailbox collisions do not merge.
- A version-30 database loses unsigned activity rows by design and reconstructs only signed rows
  from `canonical_events`.
- Two active account devices receive and project account-addressed activity through ordinary
  outbox/inbound handling, while revoked, unrelated, peer-addressed, and public attempts fail
  closed.
- Worst-case escaped and multibyte title/body input is valid UTF-8, explicitly marked truncated
  when changed, and signs below the 64 KiB wire limit.
- Progress retention is exactly the latest 200 reducer-ordered values per full source/provider
  session after live append, restart, and rebuild, without deleting canonical events.
- The legacy read API returns at most 1,000 chronological canonical projections. No public write RPC
  remains, and existing message-only state and behavior are unchanged.
- Relevant store, bridge, RPC, client, event, and transport tests pass under normal and race runs;
  `go test ./...`, `go vet ./...`, and `git diff --check` pass.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## 2026-08-25 — Lossless bounded bridge activity persistence and canonical ordering

Replaced the drop-on-full bridge channel with a mutex-protected 64-item FIFO/coalescing buffer. Durable output, terminal status, and completed command/file/tool work now backpressure until accepted; running/plan/diff/progress snapshots replace the same pending logical key at the tail, while new keys backpressure and cancellation unblocks waits. One relay-wide timeline is assigned before buffering so ready, output/activity, successive work, and stopped status retain deterministic order; persistence uses a relay-owned cancellable context and normal shutdown drains accepted work. Tests exceed capacity, prove lossless terminal delivery, bounded latest-snapshot coalescing, tail ordering, cancellation, output/activity timing, partial-write restart reconciliation, rebuild stability, and race safety.

### Original plan entry

## Lossless bounded bridge activity persistence and canonical ordering

Replace the bridge's drop-on-full persistence queue with one serialized canonical output/activity
path. Durable terminal and completed records apply cancellation-aware backpressure; replaceable
plan/diff/running/progress values coalesce by logical key, and a full buffer with a new key applies
backpressure. Preserve output/activity relative order, drain accepted durable/latest coalesced work
on shutdown, retain transient provider noise only ephemerally, and add overload, cancellation,
reconciliation, deterministic-ID, and race tests beyond the current 64-entry capacity.

Scope:

- Replace the channel/default-drop queue with an explicitly bounded FIFO/coalescing buffer shared
  by canonical assistant output and canonical harness activity. A work item normalized from one
  provider event remains indivisible and publishes output then activity in that stable order.
- Treat assistant output, failed/interrupted status output, terminal operation activity, and
  completed command/file/tool activity as durable. Enqueue them with backpressure until capacity
  is available or the relay ingestion context is canceled; never drop an accepted durable item.
- Treat running operation status, plan, diff, and progress as replaceable snapshots. When the same
  full provider/session/operation/kind/item logical key is pending, remove the older value and append
  the newer value at the tail so its position reflects provider event order. If no matching key is
  pending and the buffer is full, apply the same cancellation-aware backpressure as durable work.
- Keep the buffer bounded by pending logical work, excluding the one item currently being
  persisted. Replacement must not grow memory, and a key that is already in flight may produce one
  persisted intermediate value followed by the latest pending value; canonical reduction still
  selects the later source sequence.
- Continue discarding token deltas, spinners, raw reasoning/model payloads, and all provider events
  that normalize to neither supported output nor supported activity before they reach the buffer.
- Allocate canonical authoring times from one relay timeline. Preserve provider occurrence order,
  make bursts monotonic at signed-second granularity, and give output/activity from the same event
  deterministic adjacent positions. Store those times on normalized work before it can wait or be
  coalesced so retries and delayed persistence cannot consult receiver clocks.
- Make output and activity persistence use a relay-owned context. Parent/worker cancellation stops
  intake and unblocks enqueue waits, while an orderly provider shutdown closes intake and drains all
  accepted FIFO/latest coalesced work. A shutdown-time persistence cancellation records a relay
  failure instead of hanging silently.
- Preserve output reconciliation: stable output IDs plus the delivery ledger must avoid duplicate
  messages, and replay after a partial output-then-activity failure must reconcile the output before
  retrying the activity. Canonical activity IDs remain deterministic for identical runtime,
  sequence, occurrence, content, and membership state.

Implementation plan:

- Add a small mutex-protected `eventBuffer` with bounded items, close/wakeup signaling, contextual
  enqueue, FIFO dequeue, and replace-in-tail behavior. Keep it local to `internal/harnessbridge` and
  test it through relay behavior rather than exporting queue mechanics.
- Classify normalized activity by durability and generate a source-complete coalescing key only for
  replaceable activity-only work. Any work containing canonical output is durable.
- Refactor relay startup, ingestion, publication, and shutdown around the buffer and separate intake
  and persistence cancellation. Pass the persistence context through message reads/creates,
  project-output creation, synchronization, and canonical activity authoring.
- Replace independent output/activity clock allocators with one relay-wide monotonic timeline and
  preassigned output `CreatedAt`/activity `OccurredAt` values, preserving ready/work/stopped order as
  well as the relative order of work that waits or is coalesced before persistence.
- Replace the drop-on-saturation test with overload tests exceeding 64 entries for terminal and new-
  key work, same-key plan/progress coalescing tests, cancellation-unblocks-backpressure tests, and
  shutdown drain tests. Add ordering, partial-failure reconciliation, deterministic-ID/replay, and
  race coverage.

Risks and decisions:

- Coalescing moves a replacement to the buffer tail. Updating in place would let a newer provider
  event jump ahead of durable work that arrived between the two snapshots and would violate the
  serialized source order.
- Capacity bounds pending work, not persisted canonical history. Once a replaceable item is in
  flight it is accepted and may persist; a newer same-key value is a distinct pending item and the
  reducer's sequence-aware latest-wins rule handles both.
- Store persistence is serialized but output plus activity are not one SQLite transaction because
  they use existing message and activity authoring APIs. Stable IDs and replay reconciliation are
  therefore required at the boundary between the two writes.
- The provider event stream is closed by `Instance.Shutdown` before normal relay teardown. Intake
  cancellation is the exceptional escape hatch for a producer blocked on a full buffer, not the
  normal mechanism for declaring accepted work drained.
- A forced persistence cancellation can only prevent an indefinite process hang; it must surface as
  relay failure. Tests for orderly shutdown release persistence and require every accepted durable
  and latest coalesced value to be present before `Done` closes.

Acceptance criteria:

- More than 64 terminal operation and completed command/file/tool records block the producer while
  persistence is unavailable and all appear after persistence resumes; none are silently dropped.
- Bursts of more than 64 plan/progress updates for one logical key remain bounded and persist the
  latest accepted source sequence/value. Bursts of distinct replaceable keys backpressure at
  capacity rather than allocating or dropping.
- Canceling relay intake unblocks a producer waiting for capacity. Normal instance shutdown drains
  all work already accepted into the buffer, including the latest pending coalesced value.
- Output and activity from one provider event, and work from successive provider events, have the
  same deterministic canonical order after live projection, restart, and full rebuild.
- Replaying identical output/activity does not duplicate output messages or canonical activity;
  replay after an injected activity failure reconciles the existing output and eventually persists
  activity without an ID collision.
- Transient unsupported provider events create no queued or canonical work. Existing operation
  tracking, message presentation/correlation, project-output routing, and activity bounds remain
  unchanged.
- `go test ./...`, `go vet ./...`, `git diff --check`, and race tests for the bridge/store/event paths
  pass.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## 2026-08-25 — Unified canonical conversation history and TUI timeline

Added a validated domain-level message/activity union and a strict, canonically ordered paged read without changing the legacy message-only history API. SQLite now persists reducer display order for messages, rebuilds it through schema version 32, and returns projected mixed history isolated by mailbox/provider/session with thread fallback remaining message-only. The new RPC and HQ client method preserve complete typed message and activity fields and fail cleanly against older servers. The TUI now loads one authoritative entry sequence, derives compatibility slices for message-only actions and activity cards, renders reducer order rather than timestamps, and keeps activity out of inbox, unread, reply, archive, draft, delivery, final-answer, and scroll-anchor behavior. Store/RPC/client/TUI tests cover strict pagination, restart/rebuild, coalescing/retention, compatibility, canonical ordering, cache invalidation, and logical anchoring; full test, vet, diff, and race suites pass.

### Original plan entry

## Unified conversation history, RPC compatibility, and TUI activity timeline

Add a typed `ConversationEntry` message/activity union with stable canonical order while retaining
legacy message-only reads. Round-trip it through domain RPC and the HQ client, then move the TUI to
the unified history without making activity an inbox/unread/reply/archive/draft/final-answer target.
Preserve activity card disclosure, logical-message scroll anchoring, 1,000-row query caps, 200
progress projections, provider/session isolation, older-client unsupported behavior, and add
message-only behavior-invariance plus timeline/restart/resize/race coverage.

Scope:

- Add a domain-level discriminated `ConversationEntry` union because domain activity already
  depends on model correlation types. Each entry carries exactly one full `model.Message` or
  `HarnessActivity`, its stable canonical event ID/display order, and an explicit kind; add a paged
  response using the existing `model.ConversationHistoryFilter` and conversation key.
- Preserve `ListConversationHistory` and `conversation/history` byte-for-byte as the legacy
  message-only read. Add a separate unified store operation, RPC method/request, and HQ client
  method so older clients continue decoding and retaining activity only through canonical
  unsupported-event compatibility, without receiving a changed response shape.
- Persist reducer display order on message rows via a schema migration and rebuild. Activity rows
  already carry the same reducer order; unified pagination must use `(display_order,event_id)` rather
  than timestamps, row IDs, or receipt order.
- Query messages in either direction between human and the selected counterparty. Include activity
  only for a provider/session conversation and match the same counterparty mailbox plus exact
  provider/session namespace. HQ thread-fallback conversations remain message-only.
- Cap each page through existing page-limit rules, validate opaque cursors strictly, return entries
  chronologically, and preserve the legacy 1,000-row activity query cap and 200-progress projection.
- Move TUI detail loading to the unified endpoint. Retain derived message/activity slices only for
  existing action, cache, and rendering code, while storing the ordered entry sequence as the
  authoritative timeline so rendering no longer re-sorts by occurrence timestamps.
- Keep conversation summaries, inbox/open/unread counts, latest/final-answer selection, reply and
  archive targets, drafts, delivery state, and compose behavior exclusively message-driven. An
  activity entry has no action ID and cannot become a logical-message scroll anchor.
- Preserve collapsed/expanded activity cards, failure/truncation disclosure, viewport-based toggle,
  resize behavior, and logical message scroll anchoring when activities are inserted, coalesced, or
  re-ordered by a rebuild.

Implementation plan:

- Add conversation entry/page types and the unified method to domain/store contracts, with helpers
  that validate the discriminated union and split entries into compatibility slices where needed.
- Migrate SQLite to add `display_order` to messages, fill it from `event.State.DisplayOrder` on every
  rebuild, and add a unified candidate query over message/activity projections. Page by canonical
  order and hydrate exact typed message/activity values without reconstructing semantics from body
  or `Details`.
- Add `conversation/entries` protocol dispatch and HQ client support; retain and test the legacy
  method and activity-list method unchanged.
- Add an authoritative entry-history map to the TUI load/update/group path. Render entry order
  directly, derive message/activity slices for existing action and cache consumers, and leave a
  compatibility fallback only for hand-built test groups without typed entries.
- Add store tests for mixed pagination/order, restart/rebuild, duplicate/coalesced activity,
  provider/session/source isolation, thread fallback, and legacy message-history invariance. Add
  RPC/client round trips and TUI timeline/action/anchor/resize/cache/race tests.

Risks and decisions:

- Message IDs and event IDs differ for schema-2 messages. Unified cursors and ordering use canonical
  event IDs, while hydrated messages retain their public message IDs for all actions.
- Coalesced activity rows mean canonical history is richer than the bounded disposable unified
  projection. This endpoint intentionally reflects projected conversation history: superseded
  snapshots and pruned progress remain in the canonical log but do not reappear in the TUI.
- A message and activity may share a signed second; reducer `display_order`, not local timestamps,
  is authoritative. Timestamps remain presentation metadata only.
- Adding a required domain store method intentionally makes new in-process implementations explicit;
  old wire clients remain compatible because the legacy RPC method and response do not change.
- TUI caches compare ordered entries as well as derived slices. Activity expansion identity remains
  logical-key based so a coalesced replacement preserves disclosure state without becoming a message
  anchor.

Acceptance criteria:

- Unified pages return mixed message/activity entries in reducer order across page boundaries and
  reproduce the same entries/cursors after restart and full rebuild, independent of arrival order.
- Provider/session/mailbox collisions remain isolated; HQ thread conversations contain no activity;
  progress remains bounded to 200 projected rows and activity-list reads remain capped at 1,000.
- Legacy `ListConversationHistory`, its RPC response, and the HQ client method return the same
  message-only data/order/cursors as before. The new RPC/client method round-trips every canonical
  activity field and complete typed message semantics.
- The TUI renders the unified order without consulting body/`Details`, preserves card disclosure and
  logical-message anchoring across reload/resize/coalescing, and never selects activity for reply,
  archive, draft, final-answer, inbox, unread, or delivery behavior.
- Existing message-only TUI/store behavior tests remain unchanged or gain explicit invariance
  assertions; focused store/RPC/client/TUI race tests, `go test ./...`, `go vet ./...`, and
  `git diff --check` pass.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## 2026-08-25 — Canonical activity synchronization conformance and documentation

Extended the real account-fanout fixture into an interleaved typed message/activity/message conversation, delivered normal encrypted outbox jobs in reverse order, suppressed duplicate wrapper notifications, and proved canonical IDs, typed semantics, activity source fields, mixed reducer order, and legacy message history converge after rebuild and restart. The comparison explicitly preserves device-local recipient/address presentation and delivery state as local facts. Added a validly signed post-revocation activity wrapper test that reaches inbound decryption and causal authorization, is rejected and quarantined, increments revoked-device diagnostics, and changes no canonical, activity, mixed-history, summary, inbox, or delivery projection; public, peer, and malformed account scopes also fail validation. Rewrote README and the event, harness, and design docs around schema 32, dual-stream conversations, account audience/privacy, durable versus replaceable persistence, tail coalescing/backpressure, deterministic ordering, partial-write reconciliation, shutdown draining, canonical versus projected retention, legacy unsigned-row loss, and message-only TUI actions. Full tests, vet, diff checks, focused consistency searches, and store/event/bridge/RPC/client/TUI race suites pass.

### Original plan entry

## Canonical harness activity synchronization, documentation, and conformance

Close the canonical harness-activity integration with transport-level convergence and privacy
conformance, then replace documentation that still describes activity as installation-local and
best-effort. Prove that two active human-account devices reconstruct the same projected mixed
conversation through the ordinary outbox, encrypted gift-wrap, and inbound reducer paths; prove a
revoked source fails closed at that same boundary. Document the dual-stream model, canonical versus
projected retention, audience/privacy rules, queue durability and coalescing, ordering, shutdown,
migration loss of unsigned legacy rows, legacy compatibility, and TUI behavior in the event,
harness, design, and top-level README surfaces.

Scope:

- Strengthen the existing account-fanout store fixture from a single activity-row assertion to a
  real mixed conversation. Author typed messages and activity in one provider/session namespace,
  prepare normal per-device outbox wrappers, receive them through `ReceiveGiftWrap`, and compare the
  complete `ConversationEntry` projection on both devices, including event IDs, display order,
  typed message semantics, every activity source/correlation field, and message-only legacy reads.
- Exercise duplicate/reordered wrapper delivery and projection rebuild in the transport fixture.
  Both devices must converge on the same entry sequence without receipt-time or wrapper-order
  influence, and duplicate wrappers/logical events must not create another message, activity, inbox
  row, or change notification.
- Add activity-specific revoked-device ingress coverage. Construct a correctly signed schema-2
  account activity from a device after its signed revocation, encrypt it for the active creator,
  pass it through the real gift-wrap receiver, and require rejection/quarantine plus unchanged
  activity, conversation, inbox/open/unread, and canonical projected state. Retain event-level tests
  for unrelated-account, peer-addressed, and public activity attempts.
- Preserve the existing canonical authoring boundary: current harness activity uses the active human
  account audience and membership frontier, creates per-active-device outbox rows, and has no public
  write RPC. Protocol-level installation-private activity remains valid only for a genuinely local
  event; public and peer-addressed activity remain invalid.
- Keep all behavior-invariance properties explicit. Activity must not change conversation summaries,
  inbox/open/unread counts, delivery facts, final-answer choice, reply/archive/draft targets, project
  message behavior, or logical-message scroll anchors. Legacy `conversation/history` and
  `activity/list` remain message-only/projected compatibility reads with their existing shapes and
  caps; `conversation/entries` is the typed mixed read.
- Correct `docs/events.md`: schema 2 also defines `harness.activity`; specify its strict neutral
  payload, source mailbox address, allowed scopes/audience, membership parents, signed-wire bound,
  reducer order/coalescing, unsupported-event retention, disposable 200-progress projection,
  1,000-row legacy activity cap, mixed-history pagination, and schema-30 unsigned-row migration loss.
- Correct `docs/harnesses.md`: replace the old local/drop-on-full queue description with the bounded
  serialized buffer, durable versus replaceable classes, replace-at-tail semantics, cancellation-
  aware backpressure, relay-owned persistence context, output-before-activity work ordering,
  deterministic preassigned timeline, reconciliation after partial writes, and orderly shutdown
  draining. Describe canonical activity synchronization and current TUI cards accurately.
- Correct `docs/design.md`: update schema and projection descriptions through version 32, add the
  dual-stream conversation read model, canonical/activity outbox fanout and authorization boundary,
  canonical-log versus disposable-projection retention, rebuild behavior, and the distinction
  between activity source identity and provider-opaque correlation.
- Correct `README.md`: describe inline synchronized activity cards and `e` disclosure without
  implying transcript synthesis; state that inbox/actions remain message-only; update schema 32,
  canonical/outbox/rebuild, privacy, and shutdown language while keeping the user-facing overview
  compact and linking detailed protocol/runtime docs.

Implementation plan:

- Refactor the account-fanout test setup only enough to exchange an interleaved message/activity/
  message timeline through real relay jobs. Add deterministic helpers for delivering selected jobs
  in reverse order and comparing normalized entry pages while preserving exact typed values.
- Add a revoked-device activity test beside existing human-device and activity transport coverage,
  using the real signer and wire codec rather than direct reducer insertion. Assert quarantine and
  `NetworkStatus.RevokedDeviceTraffic` where the receiver classifies the source as revoked.
- Audit and extend behavior-invariance assertions around the transport fixtures; reuse existing
  reducer, store, bridge, RPC/client, TUI, migration, retention, and wire-limit tests instead of
  duplicating lower-level cases already proven.
- Rewrite the stale documentation sections with one vocabulary: canonical event log, projected
  conversation entry, message stream, activity stream, durable work, replaceable snapshot,
  provider/session namespace, source mailbox address, human-account audience, and logical-message
  action/anchor.
- Run targeted shuffled/duplicate/rebuild, account authorization, unsupported schema, signed-wire,
  bridge overload/shutdown, legacy history/RPC, unified history, TUI action/anchor, and migration
  suites before the repository-wide verification and race matrix.

Risks and decisions:

- Absolute reducer display indexes can include other canonical conversation events. Convergence is
  judged by the exact shared entry projection produced from the exchanged canonical set; fixtures
  must deliver all prerequisite membership and conversation events before comparing devices.
- Wrapper receipt order may be non-topological. Missing-parent events are retained and a later
  canonical append rebuilds them; the test must compare the final state, not require each reversed
  intermediate delivery to project immediately.
- A revoked device can still cryptographically create and encrypt bytes. Security depends on local
  audience routing plus causal membership authorization during inbound reduction, so the test must
  reach `ReceiveGiftWrap` and inspect the fail-closed result rather than stop at validation.
- Canonical retention and projected retention intentionally differ. Superseded snapshots and old
  progress events remain signed canonical history while the disposable activity table and unified
  TUI history expose only coalesced winners and the newest 200 progress entries.
- Documentation must describe the implemented current writer: account-addressed activity fans out
  to active human devices. Installation-private is a valid protocol scope, not a promise that the
  current bridge silently downgrades account conversation telemetry to local-only state.

Acceptance criteria:

- Two active account devices receive typed messages and every activity kind needed by the fixture
  through ordinary outbox/gift-wrap/inbound handling and return identical mixed conversation entry
  order and semantics after duplicate/reordered delivery, restart, and full projection rebuild.
- A post-revocation schema-2 activity wrapper is rejected and quarantined as revoked account
  traffic, increments the revoked-traffic diagnostic, and changes no activity, conversation entry,
  summary, inbox/open/unread, or delivery projection.
- Existing unrelated-account and revoked reducer tests, peer/public validation tests, old-schema
  unsupported-byte retention, signed-wire truncation, migration discard, coalescing/reorder,
  progress/query caps, and message behavior-invariance tests remain green.
- `docs/events.md`, `docs/harnesses.md`, `docs/design.md`, and `README.md` agree that activity is a
  canonical synchronized non-message stream with projected retention, exact privacy/audience rules,
  bounded lossless persistence classes, deterministic order, drain semantics, and message-only TUI
  actions.
- Focused store/event/bridge/RPC/client/TUI tests and race runs pass, followed by `go test ./...`,
  `go vet ./...`, `git diff --check`, and documentation consistency searches with no stale local-only,
  drop-on-full, schema-30-current, 64-KiB activity-body, or timestamp-sorted timeline claims.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.


## Completed: Durable harness activity conversation stream — final integration audit

Completed the typed schema-2 message foundation and converted harness activity into a signed,
authorized canonical event stream with deterministic reduction, bounded lossless bridge
persistence, account-device synchronization, projection rebuilds, and a unified non-actionable
conversation history. Preserved legacy message-only behavior and unsupported-event retention,
updated the TUI and protocol documentation, and closed the final audit with exact 1,000-row read
cap coverage plus full-field fake-provider rebuild/restart equivalence. All focused and
repository-wide tests, race suites, vet, build, static boundary searches, and diff checks pass.

## Durable harness activity conversation stream — final integration audit

Harness activity is currently an installation-local, best-effort SQLite projection. The harness bridge can permanently drop activity when its bounded persistence queue fills, and other devices cannot reconstruct the missing timeline. Convert activity into a typed signed canonical event stream that shares transport, persistence, authorization, replay, and deterministic ordering with messages while remaining a distinct, non-actionable entry type.

The user-facing model is one provider-namespaced harness-session conversation containing two semantic streams:

- messages, which retain inbox, unread, reply, archive, delivery, and final-answer behavior;
- harness activity, which appears inline in conversation history but never creates an inbox row, unread count, reply/archive target, draft target, or message delivery claim.

Implement the following:

- Build on the typed message semantics and schema-2 compatibility framework established above. Reuse the same harness-neutral provider/session/operation/item correlation representation; neither activity nor conversation code may reconstruct correlation or presentation from message body or `Details`.
- Add a canonical `harness.activity` event type and typed payload in `internal/event`. The payload must contain only harness-neutral fields: provider, session, operation, optional item ID, activity kind, status, bounded title/body, truncation state, occurrence time, runtime-lifetime identity, and the harness event sequence. Continue supporting operation status, plan, diff, completed command, completed file change, completed tool call, and progress. Do not place Codex method names, JSON-RPC shapes, raw provider payloads, or message technical metadata in canonical activity types.
- Give every projected entry a stable canonical event ID and associate it with the originating full mailbox address plus provider-namespaced session identity. Use operation and item IDs only as provider-opaque correlation fields. A provider/session collision across two providers must not merge conversations or activity.
- Validate kind-specific requirements, statuses, identities, scopes, UTF-8, and bounds in `internal/event/validate.go`. Enforce the actual 64 KiB signed-wire limit, including JSON escaping and envelope overhead; adjust activity body limits or truncate dynamically rather than assuming the existing 64 KiB local body limit fits.
- Authorize activity as private conversation telemetry. It may be installation-private for a genuinely local-only conversation or account-addressed to the same human account/audience as the associated agent or project conversation. It must never be public or automatically peer-addressed. Account-addressed activity must use normal membership parents and encrypted per-device outbox fanout. Reject unauthorized activity from revoked or unrelated installations.
- Replace direct projection writes from `internal/harnessbridge/events.go` with a store operation that signs and appends canonical activity. Remove or narrow the public `activity/upsert` RPC so callers cannot bypass canonical validation; retain a read API for conversation history as needed.
- Keep the bridge persistence path bounded without silent loss:
  - terminal operation states and completed command/file/tool records are durable and apply cancellation-aware backpressure;
  - plan, diff, running-state, and progress snapshots may replace an older pending value with the same logical key before it is signed;
  - when a bounded coalescing buffer has no matching key to replace, apply backpressure instead of dropping a new key;
  - token deltas, spinners, raw reasoning, raw model responses, and other transient provider noise remain ephemeral and need not become canonical events;
  - orderly shutdown drains accepted durable work and the latest accepted coalesced value before teardown.
- Preserve canonical output ordering. Activity and canonical assistant output normalized from the same harness event must enter one serialized persistence path, and their relative timeline order must be deterministic after restart and on every device.
- Extend reduction so arrival order, duplicate delivery, and replay cannot change the result. Derive `harness_activities` entirely from projected canonical events, clearing and rebuilding it during a canonical projection rebuild. Choose coalesced winners using canonical causal/display order and stable source sequence, never SQLite receipt order or the receiving node's clock.
- Preserve the existing logical projection rules:
  - operation, plan, and diff are latest-wins snapshots per conversation/operation;
  - repeated item/progress keys coalesce deterministically;
  - completed command/file/tool records and terminal operation states remain durable history;
  - retain only the most recent 200 projected progress records per provider session and cap activity queries at 1,000 chronological entries;
  - retain canonical events under the existing canonical-log policy even when older entries fall out of the disposable projection.
- Do not manufacture signed history from legacy unsigned `harness_activities` rows. The schema migration may discard those best-effort rows and rebuild from canonical activity events; document that compatibility choice.
- Add or extend a typed read-side union such as `ConversationEntry = MessageEntry | HarnessActivityEntry`. `MessageEntry` must carry typed message semantics and technical sections unchanged; the union must not derive them from text. Conversation history should return both kinds with a stable order derived from canonical reduction. Inbox/conversation summaries must continue to be calculated exclusively from messages. Preserve legacy message-only clients and have older binaries retain the new event bytes as unsupported canonical events.
- Update the TUI to consume the unified ordered history while preserving its existing collapsed/expanded activity cards, failed/truncated disclosure, logical-message scroll anchoring, drafts, final-answer presentation, and message-only reply/archive targeting.
- Update `docs/events.md`, `docs/harnesses.md`, and `docs/design.md` to describe the dual-stream conversation model, synchronization audience, durability classes, ordering, projection retention, privacy boundaries, and the fact that canonical history and projected retention are separate concerns.

Expected implementation areas include:

- `internal/event/{event.go,validate.go,reducer.go}` and their tests;
- `internal/domain/harness_activity.go`, conversation-history types, and change topics;
- `internal/store/{sqlite.go,harness_activity.go,transport.go}` plus schema migration and rebuild tests;
- `internal/harnessbridge/events.go` and overload/shutdown tests;
- `internal/domainrpc`, `internal/hqclient`, and compatibility tests;
- `internal/tui/{activity.go,tui.go}` and timeline tests;
- the canonical-event and harness design documentation.

Completion requires tests proving:

- every activity kind validates, signs, projects, and rebuilds identically;
- shuffled arrival, duplicate delivery, restart, and full rebuild produce byte-for-byte equivalent activity projections and identical conversation order;
- terminal activity is not lost when persistence is blocked past the current 64-entry queue capacity;
- plan/progress bursts coalesce to the latest accepted value without unbounded memory growth;
- two active human-account devices converge on the same inline message/activity timeline through the existing outbox and inbound canonical-event paths;
- revoked, unrelated, peer, and public activity attempts fail closed;
- provider-local session-ID collisions remain isolated;
- provider/session/operation/item association and conversation ordering do not consult message body or `Details`;
- worst-case escaped and multibyte payloads stay valid UTF-8 and below the signed-wire limit with explicit truncation;
- activity changes never alter inbox rows, open/unread counts, final-answer selection, replies, archives, delivery claims, or drafts;
- existing message-only history and legacy unsupported-event behavior remain compatible.

Final audit execution:

- Map every requirement above to its committed implementation and at least one focused test across
  event, store/transport, bridge, RPC/client, TUI, migration, and documentation. Treat preceding
  phase commits as evidence, but rerun the behavior at the current stack tip.
- Close the remaining quantitative read-bound gap with an efficient canonical batch fixture that
  projects more than 1,000 durable activity records in one append and proves `activity/list` returns
  exactly the newest 1,000 in canonical chronological order even when a larger limit is requested.
- Strengthen the fake-provider all-kind integration fixture to compare every projected activity
  field before and after an explicit full projection rebuild and daemon-owned bridge restart, not
  merely the row count. Preserve the later new-runtime replay assertion separately.
- Re-run static boundary searches: no bridge direct SQL activity writes, no public activity write
  RPC, no conversation/TUI correlation reconstruction from message body or `Details`, no stale
  local-only/drop-on-full/schema-30-current documentation, and no provider-specific vocabulary in
  canonical activity types.
- Execute focused normal and race suites for event, store/transport, bridge, RPC/client, and TUI;
  then run repository-wide tests, vet, diff checks, and the project build. Inspect the final stacked
  diff and commit history so the audit does not accidentally absorb ignored PLAN/COMPLETED files.

Audit decisions:

- Cross-device equality means canonical event IDs, reducer order, typed message semantics, and full
  activity projection. Message recipient installation presentation, resolved address labels/kinds,
  and delivery state are device-local facts and are normalized only in convergence assertions.
- The 1,000-row test uses one batch of valid signed canonical events and one reducer rebuild; issuing
  1,001 public authoring calls would test quadratic fixture cost rather than the read contract.
- No production rewrite is required when a clause already has direct code and test evidence. Any
  newly observed mismatch is fixed in scope before the umbrella can be archived.

Final audit acceptance:

- The all-kind fake-provider projection is byte-for-byte stable across explicit rebuild and restart,
  and the canonical legacy activity read enforces its exact 1,000 newest-row cap.
- Every original completion bullet has current code/test evidence; static searches find no bypass or
  stale semantic claim, and all focused/full normal, race, vet, build, and diff checks pass.
- `PLAN.md` contains no remaining task after this entry is archived, and the active goal is marked
  complete only after the final commit and ignored completion record are verified.

Run `go test ./...`, relevant race tests for the bridge/store/TUI, `go vet ./...`, and `git diff --check`.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they
don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from
the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other
marker. The task and its related subsections should no longer appear in the plan file at all. The
plan file should not have any sort of "Done" section. Then append a new entry to the completed file
at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
   preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update
those. If new future work items were discovered, add them. If the plan file or completed file is
outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
other changes.

## Mouse-wheel scrolling by hovered TUI pane — completed

Implemented coordinate-routed three-row mouse-wheel scrolling for the inbox and message panes, shared cell-motion view configuration, inert modal and non-scrollable regions, preserved focus and composer bindings, comprehensive regression coverage, and updated operator documentation.

## Mouse-wheel scrolling by hovered TUI pane

Add native vertical mouse-wheel navigation to the HQ TUI. Route each wheel event by the pointer's terminal coordinates so the hovered inbox or message pane scrolls without changing keyboard focus, entering compose mode, or disturbing an active draft/reply binding.

- Add failing tests in `internal/tui/tui_test.go` before implementation. Cover:
  - normal views request `tea.MouseModeCellMotion`;
  - wheel up/down over the inbox moves its selection by three rows, clamps at both ends, and leaves `paneFocus` unchanged;
  - inbox selection changes use the existing message-viewport reset and context/history-loading behavior when not composing;
  - wheel up/down over the message pane moves by three rendered lines through `scrollMessagePane`, remains bounded, marks a real movement as manual, and retains the existing logical message anchor behavior;
  - exact pane boundaries route correctly using the zero-based `Y` coordinate from `responsivePaneLayout`;
  - reply-pane, help-row, out-of-bounds, horizontal-wheel, blocking-connection, recipient-picker, project-setup, and agent-manager events are no-ops;
  - scrolling a pane under the pointer never changes focus or the active composer's answer/draft association.
- In `internal/tui/tui.go`, encapsulate mouse-wheel routing in a focused helper instead of expanding the main update loop with duplicated navigation logic. Use a named three-line wheel-step constant, reject unsupported directions and coordinates, and reuse the existing inbox-selection/context and message-scroll paths so mouse and keyboard behavior cannot drift.
- Set `tea.View.MouseMode` to `tea.MouseModeCellMotion` for every TUI view state. Keep alternate-screen configuration DRY when applying the shared view settings.
- Do not add independent inbox offset state, click handling, hover focus, horizontal scrolling, modal-list scrolling, or reply-textarea wheel scrolling in this task.
- Update the README's TUI controls and scrolling description to explain pane-under-pointer wheel behavior and note that terminal-native text selection may require Shift while mouse reporting is enabled.
- Run `go test ./internal/tui` and `go test ./...`.

Acceptance criteria: vertical wheel input scrolls the hovered inbox or message pane by three rows with correct boundary clamping and existing anchor semantics; keyboard focus and compose state never change because of scrolling; unsupported locations, directions, and modal states are inert; no new dependency is introduced; and all tests pass.

Implementation plan:

- Modify `internal/tui/tui_test.go` first with table-driven and focused behavioral tests for the
  shared view settings, exact zero-based pane boundaries, inbox and message movement/clamping,
  unchanged focus and compose binding, context command behavior, and all required no-op states.
- Modify `internal/tui/tui.go` to add a named three-row wheel step, one focused mouse-wheel router,
  and one DRY view constructor that applies alternate-screen and cell-motion mouse settings to
  normal and agent-manager views. Reuse `resetMessageViewport`, `withContextCommand`, and
  `scrollMessagePane`; do not introduce separate offsets or modal mouse behavior.
- Modify `README.md` to document hovered-pane wheel scrolling and the Shift modifier commonly
  needed for terminal-native selection while mouse reporting is enabled.
- Run the new focused tests first, then `go test ./internal/tui`, `go test ./...`,
  `go test -race ./internal/tui`, `go vet ./...`, `go build ./...`, and `git diff --check`.

Risks and decisions:

- Bubble Tea mouse coordinates are zero-based, while the rendered panes are vertically stacked.
  Route inbox rows `[0,inboxHeight)`, message rows
  `[inboxHeight,inboxHeight+messageHeight)`, and treat reply/help/out-of-bounds rows as inert.
- Bubble Tea sends mouse events through the normal update path after the renderer callback. Handle
  `tea.MouseWheelMsg` directly and do not install an `OnMouse` loopback callback.
- While composing, inbox-wheel selection may move but must not reset/rebind the active detail or
  draft; only non-compose inbox changes run the existing viewport/context-loading path.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.


## 2026-08-26 — Lawful causal reduction algebra and pure reducer seams

Implemented a standard-library semilattice/set/causal-frontier core with law tests, introduced SQL-free event-state contracts and architecture guards, split canonical wire validation/signing from pure projection reduction, and made the existing `event.Reduce` API a compatibility facade. The full suite, focused race tests, vet, build, and diff checks passed; commit `9080736` records the implementation.

### Original plan entry

## Lawful causal reduction algebra and pure reducer seams

Establish the functional core that every later schema-3 and incremental projection change will use.
This task changes internal structure without changing the current canonical wire or SQLite write
behavior, so it remains independently reviewable and the existing system stays green.

### Algebra and typed causal model

- Add `internal/reduction` with an immutable/copy-on-write `Set[K comparable]`, an explicit
  `JoinSemilattice[T]` dictionary (`Empty`, `Join`, `Equal`), generic folds, causal relation
  helpers, and deterministic frontier/maxima operations. Use only the standard library; do not add
  a reflection registry or external functional-programming dependency.
- Add branded event/resource identifiers at the reduction boundary so event IDs, aggregate keys,
  and projection keys cannot be accidentally interchanged as untyped strings.
- Document the algebraic laws beside the production abstractions and provide reusable test helpers
  for identity, associativity, commutativity, idempotence, duplicate tolerance, and
  chunk/permutation invariance.

### Pure reducer decomposition

- Introduce `internal/eventstate` as the SQL-free domain reduction package. Define a read-only
  causal query interface, immutable decoded fact inputs, layered validity/readiness/authorization
  results, projection support/provenance, and typed projection deltas.
- Extract concrete pure reducers from the monolithic mutable `event.State` implementation for
  mailbox/installation state, agents/sessions, peers/shares, human accounts/devices,
  messages/message state/threads, and harness activity. A reducer may not perform SQL, signing, RPC,
  transport, logging, clock reads, or mutate caller-owned input.
- Keep the current `event.Reduce` API as a compatibility facade over a full-set batch composition
  of those concrete reducers. Preserve current schema-1/2 behavior and exact observable projections
  in this task; protocol semantics change in the next queued task.
- Reuse `projectstate.Apply` as the pure project transition function and define the adapter seam by
  which authoritative and replica project facts will join the common reducer contract later.

### Tests and acceptance

- Write failing algebra law tests first, followed by generated/shuffled DAG tests for causal
  frontiers, missing parents, duplicates, and concurrent maxima.
- Port existing reducer characterization tests to assert the facade and extracted reducers return
  identical records, messages, threads, activities, agents, accounts, peers, shares, and ordering.
- Add an architecture test that rejects imports from SQL/store, RPC, TUI, signing, or transport
  packages into `internal/reduction` and `internal/eventstate`.
- Run `go test ./...`, `go test -race ./internal/reduction ./internal/eventstate ./internal/event`,
  `go vet ./...`, `go build ./...`, and `git diff --check`.

### Files

- Create `internal/reduction` and `internal/eventstate` production and test files.
- Refactor `internal/event/reducer.go` and its tests into the facade over the new core.
- Touch `internal/projectstate` only where a narrow adapter or immutability fix is required.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.


## 2026-08-26 — Schema-3 causal authority and clean protocol break

Made canonical schema 3 the sole accepted format, removed legacy schema decoding and the SQLite
migration ladder, bumped domain wire/SQLite to 7/33, and made old non-empty databases fail with
reinitialization guidance. Replaced trust/share authorization with local peer bindings and
directional mailbox grant/revoke/observation capabilities, added explicit account authorities,
receiver observation frontiers, remove-wins conflict behavior, full relay/pairing/bootstrap
coverage, and updated protocol documentation. The recovered 84-message TUI Work transcript is
stored in `TUI-Work-thread.md`. Vet, the full test suite, build, and diff checks passed; commit
`316dd76` records the implementation.

### Original plan entry

## Schema-3 causal authority and clean protocol break

- Replace exported raw type/payload pairings with validated schema-3 typed facts and constructors.
  Schema 3 is the only emitted or accepted canonical schema; remove schema-1/2 decoding,
  `legacy_message.go`, legacy detail parsing, and compatibility tests.
- Add explicit typed authority references that must be a validated subset of causal parents. Derive
  aggregate/resource keys from typed payloads rather than duplicated strings or presentation text.
- Replace global peer trust/share authorization with peer-delivered mailbox capabilities:
  `mailbox.access.grant` authorizes one peer installation/signer for one target mailbox;
  `mailbox.access.revoke` descends from the grant/frontier; peer-addressed actions reference the
  matching grant. Traffic before revocation stays authorized, traffic after or concurrent with a
  revoke fails closed, and a later causally descended grant restores access.
- Separate cryptographic validity, causal readiness, historical authorization, and current local
  routing/block policy. Blocking a peer stops new transport without hiding earlier authorized
  history. Convert account membership to the same explicit authority-reference mechanism.
- Codify resource conflict rules: remove-wins revocation/rejection/retirement, archive over
  concurrent restore, causally later restore reopening, retained answer sets, independent
  cancellation, deterministic activity winners, and explicit unique-root conflicts.
- Set canonical schema to 3, local domain wire to 7 where the event/domain DTOs change, and SQLite
  schema to 33. Delete the 7→32 migration ladder. A non-empty older database fails startup; there is
  no migration/export/translation path. The operator manually archives/removes old databases and
  reinitializes/re-pairs. Keep the provisional Nostr kind and lifecycle wire version unchanged.
- Update protocol/design/Nostr documentation and validate fresh bootstrap, peer capability exchange,
  account pairing, relay round trips, old-version rejection, and all conflict cases.

### Implementation map and test order

- Start with schema-3 fixture and validation tests in the wire package, then causal authorization
  reducer tests, then store/transport integration tests. Only after they fail for the intended
  reasons should writers and database bootstrap change.
- Change the canonical types/validation in `internal/eventwire`, authority and conflict semantics in
  `internal/eventstate`, and the capability authoring/routing paths in `internal/store`. Update local
  DTOs only where schema-3 types cross RPC; keep the lifecycle protocol unchanged.
- Replace the SQLite schema definition and configuration path in one change after protocol tests are
  green. Preserve only fresh schema-33 creation and same-version reopen; delete migration SQL and
  legacy fixtures rather than adapting them.
- Risks to test explicitly: reciprocal peer access is directional, a revoke must still be deliverable
  before local blocking disables routing, missing capability parents remain unresolved rather than
  unauthorized, and account authority must not be inferred from arbitrary causal parents.

## 2026-08-26 — Incremental causal index and core projections

Replaced the projection checkpoint with reducer-versioned metadata and durable causal, authority,
waiter, resource, frontier, support, and layered reduction indexes. Normal ingestion now inserts and
indexes exact facts transactionally, computes a directional fixed-point impact closure plus read-only
support ancestors, runs the shared pure reducers only over that closure, and upserts affected core
projections without clearing them or scanning the canonical log. Late parents wake descendants;
account and installation support remains directional so unrelated messages are not rewritten. The
batch reducer and projection clears remain confined to explicit/startup repair. Frontier and account
authoring queries now use indexed resource closures. Added startup-version repair, atomic fault,
late-parent, no-clear, current-index, and unrelated-generation regression tests. Project replay was
also made project-scoped to preserve unrelated projects and unsigned operation state. Vet, all tests,
pure-core race tests, build, and diff checks passed.

### Original plan entry

## Incremental causal index and core projections

- Replace `projection_checkpoint(event_count, rebuilt_at)` with reducer-versioned metadata and
  durable indexes for forward/reverse causal edges (including missing parents), unresolved waiters,
  event-to-resource membership, authority dependencies, aggregate frontiers, layered status/reason,
  and projection support generations.
- Implement one transactional fixed-point ingestion loop: insert exact signed facts and indexes,
  seed new facts/waiters/resource peers/reverse dependents, reclassify only that closure, run the
  affected pure reducers, diff prior values, patch changed projection rows, reconcile affected
  outbox/change topics/checkpoint, then commit before notification.
- Make installation/mailbox/agent/session, capability, account/device, message/message-state/thread,
  and activity projections incremental. A late parent resolves waiting descendants; concurrent
  revocation retracts losing projections; regrant/restore reprojects only supported descendants.
- Remove canonical full scans from normal frontier, authority, parent, message-state, and agent
  operations. Normal local and remote writes must not call the batch reducer, clear projection
  tables, update every event status, or rewrite unsigned leases/receipts/attempts/runtime state.
- Retain an explicit offline repair path that clears only rebuildable indexes/projections and folds
  all schema-3 facts through the batch oracle. Startup runs repair only for missing/mismatched
  reducer metadata.
- Add transaction fault tests at every ingestion boundary plus SQL-trace tests proving an unrelated
  append performs no all-event query, projection-table clear, or unrelated-row write.

## 2026-08-26 — Incremental projects, outbox, and conversation ordering

Added typed project, resource, agent, and accepted-message keys to causal indexing so related
projects enter the same affected closure while unrelated projects remain untouched. Authoritative
and replica replay, project-input discovery, legacy projection support, cleanup, and command-derived
state are now project-scoped; unsigned runtime/worktree/retirement data and unrelated projects are
preserved. Outbox reconciliation remains limited to the routing closure and retains exact bytes and
attempt state. Removed persisted global `display_order` from messages and activities. Conversation
pages now derive deterministic parent-before-child order from causal edges, with immutable time/ID
tie-breakers, conversation-local positions, and strict event-anchored cursors. Standalone activity
ordering/retention uses immutable occurrence time and event ID. Added causal clock-skew ordering and
schema regressions; the full cross-package suite, vet, build, and diff checks passed.

### Original plan entry

## Incremental projects, outbox, and conversation ordering

- Route project events through typed project/resource/agent aggregate keys. Incrementally replay only
  the affected project chain through `projectstate.Apply`; preserve the last unambiguous head on a
  fork and re-evaluate global agent/resource exclusivity through their own aggregate keys.
- Incrementally maintain authoritative/replica projects, project inputs, acceptances, dispatch
  records, output provenance, queued work, resource claims/health, and command results without
  clearing project tables or disturbing unsigned runtime/worktree/retirement operations.
- Reconcile only outbox rows affected by a new fact or capability/account routing frontier. Preserve
  exact bytes and existing relay-attempt state for unaffected rows.
- Remove global dense `display_order`. Persist immutable conversation sort components and derive a
  deterministic parent-before-child order within a requested conversation. Update local wire-7
  entry DTOs and cursors to conversation-local positions; ordering is presentation-only and cannot
  affect authorization or winner selection.
- Add differential tests for project forks/repair, remote replicas, cross-project exclusivity,
  project input/output, outbox fanout/revocation, late conversation entries, activity retention,
  and paginated mixed message/activity history.

## 2026-08-26 — Differential incremental-reduction conformance

Added a reusable differential oracle that snapshots the reducer-owned SQLite boundary and fully
paged conversation APIs, performs an offline batch rebuild over the identical canonical log, and
reports the first divergent normalized row. Added deterministic signed-DAG schedules covering
prefixes, reverse and late dependencies, seeded shuffles, duplicates, message lifecycle state,
capability revoke/regrant, human-device membership, activity coalescing, project forks, and global
resource/agent conflicts. The harness exposed a real incremental divergence: a later revocation
reclassified a previously projected message but left its typed row visible. Incremental projection
now retracts impacted messages, threads, and activities whose canonical facts cease to project.
Build, vet, the full test suite, differential tests, race tests, and diff checks pass.

### Original plan entry

## Differential incremental-reduction conformance

- Build a reusable differential harness that feeds generated DAG prefixes, shuffled permutations,
  duplicates, and missing dependencies through incremental ingestion and compares event status,
  frontiers, projections, outbox, projects, and conversation pages with a clean batch rebuild.
- Cover late capabilities, revoke/action concurrency, regrant, archive/restore/reject, account
  membership, activity coalescing, project forks, and cross-project resource/agent conflicts.
- Require incremental and batch results to converge for every supported arrival order and duplicate,
  with diagnostics that identify the first divergent event, resource, or projection row.

Implementation plan:

- Add `internal/store/reduction_conformance_test.go` with a reusable test-only harness. It will run
  deterministic prefix, reverse, seeded-shuffle, duplicate, and late-dependency schedules against
  fresh SQLite fixtures. At every requested checkpoint it will snapshot incremental state, invoke
  the existing offline `Rebuild` oracle over the same canonical log, snapshot again, and report the
  first differing table, API page, or normalized row.
- Make the snapshot contract explicit. Compare canonical reduction status/reason, layered event
  status, causal/authority/waiter/resource indexes, generation-free frontiers and projection
  support, core mailbox/agent/message/thread/account/access projections, stable outbox routing and
  state, authoritative and replica project projections, harness activities, and fully paged
  conversation summaries/entries. Exclude generation counters, repair timestamps, mutation/change
  receipts, delivery leases, relay attempts, runtime attempts, resource-health observations, and
  unsigned local drafts because batch reduction does not own them.
- Add `internal/store/reduction_conformance_scenarios_test.go` with small deterministic signed DAG
  builders and scenario tests for late parents, duplicates, archive/restore/reject, capability
  arrival/revoke/action/regrant, human-device membership, activity coalescing, project forks, and
  cross-project resource/agent conflicts. Reuse store signing, project payload, and account helpers
  rather than duplicating production reduction logic.
- If the harness finds a real divergence, make the narrowest production correction in the owning
  reducer/projection file—principally `internal/store/causal_index.go`, `internal/store/sqlite.go`,
  or `internal/store/project_projection.go`—and retain the failing schedule as a regression test.

Test strategy:

- Write the normalized snapshot and one late-parent/duplicate scenario first, prove that deliberate
  projection corruption produces a useful first-row diagnostic, then add each causal domain.
- Use fixed timestamps, UUIDs, secrets, and PRNG seeds so failures reproduce exactly; bound the
  schedule set rather than enumerating factorial permutations.
- Run the focused conformance suite repeatedly and under the race detector, followed by build, vet,
  and the full repository suite.

Risks and decisions:

- A same-database before/after oracle deliberately compares reducer-owned results, not transport or
  workflow side effects that `Rebuild` preserves. The table allowlist makes that boundary reviewable.
- Project input reconciliation may append deterministic acceptance facts while ingesting an input.
  The harness snapshots the resulting canonical log before rebuilding, so the oracle receives the
  exact same facts and tests projection convergence rather than re-running external workflow intent.
- Conversation cursors and display order are compared through public paged APIs as well as backing
  rows, catching ordering bugs that a table-only snapshot would miss.
- The scenario suite will stay small enough for normal CI; large-history cost belongs in the next
  bounded-work and benchmark phase.

## 2026-08-26 — Bounded-work, restart, and protocol conformance gates

Added structural closure-size, unchanged-generation, indexed-query-plan, and large-history benchmark
gates plus a schema-3/database-33/domain-wire-7 reopen and repair matrix. Ordinary ingestion now
advances metadata without recounting history, preserves only affected mailbox/agent operational
state, prunes activity by affected partition, scopes observation lookup to relevant grants, skips
irrelevant project-input/command scans, and resumes pending commands at relay ingress and node
startup. Human account authoring now reads the reducer-maintained active creation/acceptance
authority projection instead of re-verifying every causally descended account message. On an Apple
M2 Pro with 10 fixed iterations, independent append improved from 12.30 ms to 2.84 ms at 32 history
entries and from 140.89 ms to 4.85 ms at 512; affected work stayed at 4 rows and post-change
allocations stayed essentially flat (~160 KB and ~2.2k per operation). Exact canonical/outbox bytes,
unsigned drafts, receipts, and relay state survive reopen and repair. Build, vet, the full suite,
focused protocol tests, store/node race suites, and diff checks pass.

### Original plan entry

## Bounded-work, restart, and protocol conformance gates

- Add large-history benchmarks and regression assertions that normal work is bounded by the affected
  closure rather than total history. Record useful baseline and post-change measurements.
- Add crash/reopen conformance, fresh bootstrap, relay, mutation retry, subscriptions, project
  command processing, repair, and durable-draft restart coverage on schema 3/database 33/domain wire 7.
- Require normal writes never to scale with total history or clear complete projection tables.

Implementation plan:

- Add `internal/store/reduction_performance_test.go` with deterministic independent-history fixtures,
  closure-size assertions using the ingestion transaction's impacted/affected tables, generation
  checks proving unrelated reductions are untouched, query-plan guards for canonical event-type
  lookups, and benchmarks at small and large history sizes. Keep setup outside benchmark timing and
  report affected rows per operation alongside time and allocations.
- Remove history-wide work from ordinary ingestion while keeping repair deliberately whole-log:
  advance projection metadata by the number of newly inserted facts instead of recounting the log;
  preserve mailbox activity, named-agent activity, and ownership only for projections present in
  the affected pure state; prune activity retention only for affected source/session partitions;
  and skip or resource-scope mailbox-observation lookup when a batch has no relevant inbound peer
  traffic.
- Give canonical project-command/status lookups an explicit schema-33 index, installed idempotently
  on same-version reopen, and avoid polling all project commands after unrelated canonical appends.
  Preserve replay after a lost response or restart by processing commands when they arrive and once
  the node has installed its runtime command handler.
- Add a focused protocol restart matrix across store/node integration tests. Assert a fresh database
  emits only schema-3 canonical events under SQLite 33, negotiates domain wire 7, preserves unsigned
  drafts and mutation receipts across reopen, resumes subscriptions after node restart, retains
  exact relay/outbox state, processes a pending project command once, and lets explicit repair
  rebuild projections without erasing operational state.

Test strategy:

- Land the closure and query-plan tests first and run the benchmark before production changes to
  record a baseline. Apply narrow fixes one source of unbounded work at a time, retaining a
  regression assertion for each discovered path, then record the post-change benchmark under the
  same machine/process conditions.
- Reuse existing node, relay, mutation, project-command, and draft fixtures rather than inventing a
  second protocol implementation. Use fixed IDs and bounded histories in normal tests; reserve the
  larger fixtures for benchmarks and one non-timing structural regression.
- Run focused store/node protocol tests repeatedly and under the race detector, then run build, vet,
  the full repository suite, and `git diff --check`.

Risks and decisions:

- Wall-clock ratios are useful measurements but flaky correctness gates. CI assertions therefore
  inspect affected-row counts, unchanged generations, and indexed query plans; benchmark numbers
  are recorded for engineering evidence only.
- Account membership changes legitimately touch the account's delivery closure, and activity
  retention legitimately touches one source/session partition. "Bounded" means proportional to
  that typed affected closure, not universally constant work.
- Same-version schema-33 reopen may add only idempotent local indexes/tables; canonical wire and
  projection semantics remain schema 3 / DB 33 / domain wire 7 with no legacy migration path.
- Repair remains the sole whole-log/whole-projection path and must preserve unsigned drafts,
  receipts, relay attempts, runtime leases, and other operational state outside reducer ownership.

## 2026-08-26 — Legacy reducer cleanup and final pure-core contract

Replaced the reducer's implicit orchestration with one named, effect-free pipeline and one stable
`event.Reduce` facade shared by dependency-closed ingestion and the complete-log repair oracle.
Removed the duplicate affected reducer, unused store reducer interface, unused generic
fact/decision/delta prototype, and duplicate message-order pass. Architecture tests now pin the
stage inventory and forbid alternate reducer entry points or direct state-package imports; facade
equivalence covers shuffled and duplicated schema-3 DAGs. Current documentation now describes
schema 33's causal support indexes, incremental projections, derived conversation order, and the
separate Nostr wrapper/canonical schema versions. Build, vet, the full suite, pure reducer race
tests, and store race tests pass; the relay node integration suite passes normally, while its
four-second asynchronous capability deadline is consistently too short under race instrumentation.

### Original plan entry

## Legacy reducer cleanup and final pure-core contract

- Remove the old monolithic reducer/rebuild write path, obsolete schema/status helpers, stale docs,
  migrations, compatibility fixtures, and transitional APIs. Keep only the batch repair oracle and
  shared pure reducers.
- Require every canonical domain to use the pure-core contract and no old compatibility path to
  remain.

Implementation plan:

- Replace the hard-coded reducer call sequence with one effect-free named pipeline in
  `internal/eventstate`. The pipeline dictionary will group control/causal readiness, peer and
  mailbox capabilities, human account membership, domain authorization, mailbox/agent projection,
  message lifecycle/thread/order, and harness activity stages. Raw wire inspection initializes an
  owned state once; every stage receives only that state and performs no SQL, signing, clocks,
  transport, RPC, or caller-owned mutation.
- Keep one reducer function for both dependency-closed incremental sets and the full batch repair
  oracle. Remove the duplicate `ReduceAffected` facade, the unused store `Reducer`/`ReducerFunc`
  abstraction, the unused generic fact/decision/delta prototype, and the duplicate message-order
  pass. Keep `internal/reduction`'s used immutable set, semilattice, fold, and causal relation laws.
- Make the final boundary mechanically reviewable: `internal/event` is the only wire/state facade;
  normal incremental and explicit repair paths both invoke its single pure reducer after selecting
  their input set; only explicit repair reads the whole canonical log or clears full rebuildable
  projections. Add architecture tests for the pipeline's complete ordered stage inventory, facade
  equivalence, the absence of transitional reducer symbols, and effectful-import exclusions.
- Remove or correct stale reducer/projection documentation. Update `docs/design.md` from the deleted
  checkpoint/schema-32/dense-order/share model to schema 33's causal indexes, reducer metadata,
  incremental projection support, capabilities, account authority, conversation-local order, and
  explicit offline repair oracle. Clarify that the Nostr rumor's schema 1 is the wrapper envelope,
  while its embedded canonical event is schema 3.

Test strategy:

- Add failing contract tests before changing production: assert the expected named stage list once,
  compare the public event facade with the direct pure core over shuffled/duplicated schema-3 DAGs,
  and statically reject `ReduceAffected`, store reducer interfaces, effectful imports, and any
  whole-log reducer call outside the explicit repair function.
- Run the differential incremental-versus-rebuild oracle after consolidation to prove both callers
  still share semantics, plus existing algebra/property and schema-3 authorization suites.
- Finish with pure-core race tests, focused store conformance/race tests, build, vet, the full suite,
  documentation searches for obsolete schema/checkpoint language, and `git diff --check`.

Risks and decisions:

- Go cannot encode Haskell typeclasses directly; the named stage interface and immutable algebra
  dictionaries provide the useful property here—explicit composition and law-testable behavior—
  without reflection, registration magic, or an external functional library.
- The pure pipeline may mutate only its newly allocated internal state. Inputs and projected slices
  remain copied at the boundary, so callers cannot observe mutation and stages remain deterministic.
- `event.Reduce` stays as the stable package facade. "Remove the old reducer" means eliminating
  duplicate/transitional entry points and orchestration, not removing the explicit full-set repair
  oracle or schema-3 rejection tests.
- Historical planning records and the recovered Alice transcript intentionally describe older
  schemas and are not product documentation; only current README/docs and production comments are
  cleanup targets.

## 2026-08-26 — Dismiss the project recipient chooser with pane-navigation keys

Tab and Shift-Tab now dismiss the project/direct-recipient chooser through the same state transition
as Escape, clearing its query and cursor, returning focus to the inbox, and preserving inbox
selection and scroll state. Focused regression coverage exercises both keys and the full TUI suite
passes.

### Original plan entry

- Pressing <tab> or <s-tab> from the [project · choose project work or direct recipient]
  pane should be equivalent to pressing <esc> from there.

## 2026-08-26 — Keep bottom-anchored message panes tailing live content

The TUI now records whether the selected message pane is at its last full viewport before either a
snapshot or history refresh. If so, it repins the refreshed pane to the new last full viewport,
including when a newly arrived message is taller than the pane; non-bottom logical anchors continue
through the existing reconciliation path. Regression coverage exercises both refresh forms, and
the focused, full, vet, repository-wide, and TUI race suites pass.

### Original plan entry

- When the message pane is scrolled to the bottom and a new message arrives, we should tail it
  (scroll with it). In other words, if the scrollbar is at the bottom, we should keep it there.

## 2026-08-27 — Rust behavior ledger and product boundary

Recorded the immutable final Go commit/tree and classified 191 externally meaningful compatibility,
algebra, identity, messaging, relay, harness, project, client, security, operations, and regression
behaviors as retain, redesign, or drop with required/deferred/excluded release disposition and a
downstream owner. Added source and command/TUI coverage indexes, retained all four former Go-plan
regressions, and accepted focused ADRs for Linux/macOS plus single-executable packaging, encrypted
identity backup and Go-state isolation, and first-release client/provider workflows. Added a
portable verifier for baseline identity, source markers, unique/valid classifications, regressions,
ADR acceptance, and unresolved markers; it failed first on absent artifacts and now passes with
Bash syntax and ShellCheck. Updated downstream roadmap packages to carry the platform, packaging,
and backup decisions. The unchanged Go baseline passes build, vet, cached tests, and a fresh full
`go test -count=1 ./...` run.

### Original plan entry

- **[design/high] Establish the Rust behavior ledger and product boundary** — Record the frozen Go
  baseline without changing it, then classify every externally meaningful capability from the
  authoritative sources as **retain**, **redesign**, or **drop** in a tracked Rust-era behavior
  ledger. Resolve the first-release feature/deferred boundary, supported operating-system surface,
  identity backup scope, CLI/TUI workflow inventory, and other product-level choices. For choices
  not fixed by the rewrite design, select a conservative first-principles default and record it in
  a focused ADR. Preserve the four former Go-plan findings as Rust requirements: causal-maximal
  regrant authority, one canonical conversation comparator, indexed pagination, and non-disruptive
  relay wakes. Complete this work when no Go-facing compatibility assumption or retained user
  workflow remains uncategorized and later tasks can rely on a stable product boundary.

  Implementation plan:

  - Create `docs/rust/behavior-ledger.md` as the traceable source of truth for the frozen Go
    baseline, source inventory, compatibility boundaries, retained capabilities, deferred scope,
    and the four inherited regression requirements. Give every behavior a durable capability name,
    a `retain`/`redesign`/`drop` classification, an explicit first-release/deferred/excluded
    disposition, and a downstream specification or work-package owner.
  - Add focused accepted ADRs under `docs/adr/` for the Unix first-release platform and single
    executable packaging boundary; encrypted identity backup with complete Go-state isolation; and
    the supported CLI, Ratatui, and managed-provider workflow boundary. Keep protocol field values,
    provider version selection, and quantitative budgets owned by their later specification tasks.
  - Add `scripts/verify-rust-behavior-ledger.sh` first and demonstrate that it fails while the
    ledger/ADRs are absent. Make it check the frozen commit/tree, unique behavior IDs, allowed
    classification/disposition values, source coverage markers, inherited regression IDs, and ADR
    references so uncategorized additions fail visibly.
  - Verify the frozen Go revision with its existing full test suite, run the ledger verifier, and
    run the repository's normal test/vet/build gates. Review the final ledger directly against
    `rust-rewrite-design.md`, `rust-port.md`, the algebra note, `README.md`, `docs/`, CLI dispatch,
    embedded agent help, and project/harness specifications before archiving this plan entry.

  Risks and decisions:

  - A ledger can appear exhaustive while combining distinct authority or recovery rules. Keep
    security, algebra, transport, runtime, client, and project behaviors in separate rows and use a
    source-coverage index rather than relying on prose claims of completeness.
  - `redesign` means the capability remains desired but its Rust semantics are specified afresh; it
    does not imply Go wire, schema, command, UI, timing, or diagnostic compatibility.
  - The recorded Go baseline is the final pre-roadmap commit and tree, not the current branch that
    contains Rust planning documents. No Go source, fixture, schema, or deployment file is changed.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.


## 2026-08-27 — Causal fact algebra and semantic fact catalog

Specified an implementation-independent causal algebra with structural and usable reachability,
typed dependencies, deferred reconsideration, exact causal frontiers, projection retraction,
historical authority, explicit conflict registers, deterministic presentation, batch reduction, and
normalized observations. Cataloged 48 canonical and remote-control fact families with complete
authority, validation, conflict, projection, retention, and observation rules. Added 115 named
acceptance scenarios covering all nine algebraic laws, authority races, domain conflicts, project
invariants, remote-control isolation, security attacks, and inherited regressions, plus a portable
completeness verifier. Both Rust-spec verifiers, Bash syntax, ShellCheck, whitespace checks, Go
build/vet, and the fresh full Go test suite pass.

### Original plan entry

- **[design/high] Specify the causal fact algebra and semantic fact catalog** — Create tracked,
  implementation-independent specifications for the add-only fact set, graph terminology,
  reachability, usability, deferred dependencies, causal maxima, explicit historical authority,
  projection retraction, deterministic conflict rules, and canonical presentation order. Catalog
  every retained fact family for identity, installation-local control, peers, mailbox capabilities,
  human accounts, conversations, activity, agents, sessions, projects, and remote control. For each
  fact define required parents and authorities, validation, unresolved behavior, conflict policy,
  projection effects, retention class, and normalized observations. Turn all nine algebraic laws,
  safety properties, and known Go defects into named acceptance scenarios. Complete this work when
  the pure reducer can be implemented without consulting Go control flow or prose with an undefined
  conflict outcome.

  Implementation plan:

  - Add `docs/rust/causal-algebra.md` to define semantic fact identity, the add-only set and merge,
    structural versus usable reachability, dependency roles, decision categories, reconsideration,
    causal frontiers, projection support/retraction, complete-batch reduction, incremental equality,
    explicit historical authority, conflict registers, and the sole presentation comparator.
    Specify normalized reducer output without importing wire, SQL, clock, transport, or runtime
    representation.
  - Add `docs/rust/semantic-fact-catalog.md` with one durable catalog ID for every retained fact or
    signed remote-control family. For each entry record its semantic payload, scope/signer, required
    parents, authority references, validation, unresolved behavior, concurrent-conflict policy,
    projection effects/support, retention, and normalized observations. Expand the tricky peer,
    revoke/regrant, human-membership, conversation/activity, agent/session, global project-claim,
    linear project-history, dispatch, and remote-control rules into implementation-ready sections.
  - Add `docs/rust/acceptance-scenarios.md` defining deterministic fixture vocabulary and normalized
    observations, then name scenarios for all nine laws, graph/dependency safety, every authority
    race, message/activity conflict and ordering, agent/session conflicts, project transitions and
    cross-project invariants, remote commands, projection retraction, and all four inherited
    regressions.
  - Add `scripts/verify-rust-causal-spec.sh` first and show that it fails while the specifications
    are absent. Make it verify catalog field completeness and unique IDs, required fact families,
    nine named laws, required attack/regression scenarios, cross-document links, allowed retention
    and protocol-class values, and the absence of unresolved markers. Extend the behavior-ledger
    verifier only if the new specifications expose a product-boundary omission.
  - Run Bash syntax and ShellCheck on both specification verifiers, both verifiers themselves,
    whitespace checks, the Go build/vet gates, and a fresh full Go test suite to prove the frozen
    scenario source remains intact before archiving this plan entry.

  Risks and decisions:

  - Semantic facts must not freeze JSON fields, Nostr kinds, SQL rows, numeric protocol limits, or
    Go type names. Protocol work will map each catalog entry explicitly later.
  - Every declared parent is a required causal dependency; authority references are typed roles
    within that set. An absent or currently unusable parent blocks semantic support, and an
    unrelated usable parent can never supply authority.
  - Safety-sensitive singleton state uses remove-wins or an explicit multivalue conflict, never a
    timestamp/fact-ID winner. Signed times and stable IDs are reserved for deterministic
    presentation after causal readiness.
  - Project histories are home-linear, while resource and agent cardinality are global projections.
    A malformed home fork or cross-project conflict is exposed and fails closed rather than being
    hidden by store transaction order.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
## 2026-08-27 — Rust workspace and dependency guardrails

Created the twelve-crate Rust workspace with a pinned Rust 1.98 toolchain, shared deny-level Rust
and Clippy policy, rustfmt configuration, and the single `hq` binary owned by `hq-node`. Added a
tested, standard-library-only walking skeleton across protocol, domain, application, reducer, and
composition boundaries. Added automated crate inventory, direct-dependency, pure-core,
provider-neutrality, and binary-ownership checks; strict cargo-deny policy; Linux/macOS native CI;
and four-target pure-core checks. Documented the workspace and contributor gates. Formatting,
architecture checks, strict Clippy, Cargo check/build/tests, cargo-deny 0.20.2, all four target
checks, Go build/vet, and the fresh full Go suite pass.

### Original plan entry

- **[foundation/high] Establish the Rust workspace and dependency guardrails** — Add the Cargo
  workspace and initial `hq-domain`, `hq-reducer`, `hq-protocol`, `hq-application`, `hq-store`,
  `hq-local-api`, `hq-relay`, `hq-harness`, `hq-codex`, `hq-tui`, `hq-node`, and `hq-testkit`
  boundaries, initially combining crates only where that improves clarity without weakening
  dependency direction. Configure rustfmt, strict Clippy policy, tests, CI, dependency auditing, and
  architecture checks that keep Tokio, SQLite, Nostr, Ratatui, filesystem, process, and provider
  dependencies out of the pure core. Establish the ADR-0001 Linux/macOS target matrix while keeping
  core crates portable without claiming Windows product support. Add a minimal in-memory walking
  skeleton proving that a domain fact can cross the intended boundaries. Complete this work with a
  clean build/test/lint run and automated forbidden-dependency enforcement.

  Implementation plan:

  - Add a virtual Cargo workspace with the twelve capability-named crates from the architecture,
    a pinned stable toolchain, edition/MSRV/license metadata, shared strict Rust and Clippy lints,
    deterministic development profiles, and no third-party runtime dependency in the initial
    skeleton. Give every crate a documented public boundary and keep `hq-node` as the only
    composition root and owner of the single `hq` binary.
  - Add the smallest typed in-memory vertical slice: `hq-protocol` converts an already trusted
    in-memory frame into an `hq-domain` fact, `hq-application` accepts the fact through a use-case
    boundary, `hq-reducer` derives a projection without I/O, and `hq-node` composes the path. Write
    unit and integration tests before the implementation, including duplicate submission and
    invalid-frame behavior, without pre-empting the next package's full validated-value model.
  - Add `scripts/verify-rust-architecture.sh` first and capture its failure while the workspace is
    absent. Make it verify the exact workspace/crate inventory, shared lint inheritance, the single
    binary owner, direct internal-dependency allowlists, and forbidden runtime/adapter/filesystem/
    process/provider vocabulary in `hq-domain` and `hq-reducer`, plus the one-way
    `hq-codex`-to-`hq-harness` boundary.
  - Add a strict `deny.toml` for advisory, license, duplicate, wildcard, registry, and Git-source
    policy. Extend CI without weakening the frozen Go gates: run Rust format, architecture,
    Clippy, build, and tests natively on Linux and macOS; check the pure core against all four
    ADR-0001 release target triples; and run cargo-deny 0.20.2 on the complete workspace.
  - Document the workspace boundary and contributor verification commands, then run the
    architecture verifier, Cargo metadata/format/check/build/test/Clippy gates, cargo-deny, all
    four core target checks where locally available, whitespace checks, and the unchanged Go
    build/vet/fresh full test suite before archiving this plan entry.

  Risks and decisions:

  - The skeleton uses only standard-library types and deliberately small fact/frame/projection
    shapes. Full bounded values, cryptographic identifiers, catalog payloads, and deterministic
    builders remain owned by the immediately following domain package.
  - Architecture checks enforce both dependency declarations and source-level forbidden imports;
    Cargo's acyclic graph alone cannot distinguish an allowed adapter dependency from accidental
    I/O in the pure core.
  - Linux and macOS native CI are product gates. Cross-target `cargo check` proves portable core
    compilation for x86-64 and ARM64 but does not pretend to exercise target-specific adapters.
  - Dependency policy starts deny-by-default for unknown registries and Git sources while allowing
    the common MIT, Apache-2.0, BSD, ISC, Unicode, and Zlib families expected by later packages.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
## 2026-08-27 — Validated domain primitives and error taxonomy

Replaced numeric skeleton identities with eleven distinct opaque 32-byte ID types and separate
signing/encryption public keys. Added bounded text, vector, root-capable set, and non-empty set
types; typed installation/mailbox addresses; timestamps and revisions; causal parents and
role-specific authority references; provider-neutral operation correlation; resource locators;
command, outcome, page, and versioned-view envelopes; and structured domain errors. Constructors
exclude empty, oversized, duplicate, and unrelated-authority states without I/O, encoding, clocks,
or randomness. Updated the walking skeleton, documentation, and architecture rule accordingly.
Public tests, compile-fail doctest, format, strict Clippy, architecture/spec verifiers, cargo-deny,
all four core targets, Go build/vet, and the fresh full Go suite pass.

### Original plan entry

- **[domain/high] Implement validated domain primitives and error taxonomy** — Replace the walking
  skeleton's placeholder identity and payload shapes with newtyped IDs, public keys, addresses,
  causal references, bounded text and collections, timestamps, correlation values, resource
  locators, generic command/outcome/view envelopes, and typed error categories. Test constructors,
  bounds, non-interchangeability, equality, deterministic ordering primitives, and invalid-state
  exclusion without wire, storage, filesystem I/O, ambient time, or random generation. Complete
  this work when the fact catalog and reducers can depend on validated vocabulary rather than raw
  strings, integers, or byte arrays.

  Implementation plan:

  - Add focused `hq-domain` modules for identifiers and keys, bounded values, time, addressing,
    causal dependency references, correlation, resource locators, command/outcome/view envelopes,
    and structured domain errors. Use private representation, fallible constructors, owned data,
    explicit accessors, and deterministic `Eq`/`Ord` only where the semantics require them.
  - Define distinct fixed-width newtypes for fact, installation, mailbox, account, agent, project,
    message, resource, command, receipt, and operation identities plus public signing/encryption
    keys. Provide byte access without textual parsing or encoding policy; keep secret-key custody,
    signatures, hashing, and serialization outside this package.
  - Define reusable non-empty bounded text and bounded unique collections; an explicit signed
    millisecond timestamp; typed local, account, mailbox, provider/session, operation, and project
    correlation; typed authority roles/references and parent sets; and scheme-tagged resource
    locators that validate their opaque canonical value without touching the filesystem.
  - Write public-contract tests first for empty/oversize/duplicate rejection, type-specific
    address construction, stable ordering, authority-parent consistency helpers, resource scheme
    separation, typed errors, and command/outcome/page behavior. Replace the skeleton's numeric
    fact identity and raw payload construction while keeping its boundary test passing.
  - Document which invariants are enforced now and which belong to protocol verification or the
    following semantic-payload package. Run format, strict Clippy, architecture, cargo-deny,
    Cargo check/build/tests/doctests, four-target pure-core checks, whitespace checks, and the
    unchanged Go build/vet/fresh full suite before archiving this split package.

  Risks and decisions:

  - Fixed-width byte identities are semantic opaque values, not a commitment to a textual or wire
    encoding. Protocol code will later prove content-derived IDs and signatures before constructing
    verified facts.
  - Generic bounded types reject invalid inputs but do not silently normalize Unicode, paths, or
    external provider identifiers; producers must supply the canonical form owned by their
    protocol or adapter.
  - Ordering on IDs and timestamps supports sets, indexes, and the specified presentation tuple;
    it never selects a winner for a concurrent semantic conflict.
  - The 48 catalog payload variants and deterministic test generators remain in the next split
    package so this commit stays reviewable and its primitives can be evaluated independently.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
## 2026-08-27 — Semantic fact payloads and deterministic test support

Replaced the text-only skeleton fact with a verified semantic envelope carrying typed author,
scope, timestamp, causal parents, authority roles, and payload. Added an exact 48-family code
catalog and one typed payload variant per normative FCT row, with canonical/remote-control
isolation and focused intrinsic validation. Code, Markdown, and constructed fixtures prove exact
bidirectional catalog coverage. Added deterministic byte/ID/key streams, explicit clocks, valid
fact builders, all-family payload fixtures, exhaustive small arrival permutations, and
shrink-friendly state-machine sequences. Updated the walking skeleton and documented the boundary.
All Cargo format/check/build/test/doctest/strict-Clippy gates, architecture/spec verifiers,
cargo-deny, four target checks, Go build/vet, and the fresh full Go suite pass.

### Original plan entry

- **[domain/high] Model semantic fact payloads and deterministic test support** — Define a typed
  payload variant for every canonical and remote-control family in the semantic fact catalog using
  only validated `hq-domain` primitives. Build deterministic key, ID, clock, random-byte, fact,
  graph, and state-machine generators in `hq-testkit`, with catalog fixtures and shrink-friendly
  construction. Test complete catalog coverage, payload-specific invalid-state exclusion,
  deterministic generation, and the ability to express every named acceptance scenario without
  raw strings or ambient time/randomness. Complete this work when later reducers and protocol code
  need no ad hoc semantic DTOs or test entropy.

  Implementation plan:

  - Replace the temporary skeleton `Fact` with a verified `SemanticFact` envelope containing an
    opaque fact ID, typed author/scope, explicit timestamp, bounded parents and authority roles,
    and a `SemanticPayload`. Keep signatures, hashes, encoding, receipt metadata, and storage state
    outside the domain envelope.
  - Organize the 48 payload variants into installation/identity, authority/account, conversation/
    activity, agent/session, project, and remote-control modules. Reuse narrow typed records for
    labels, message presentation, lifecycle state, resource health, assignment/runtime outcomes,
    and command stages while retaining a one-to-one `FactKind` and enum variant for every FCT ID.
    Encode required intrinsic exclusions in fallible constructors rather than reducer branches.
  - Add a catalog table in code mapping every `FactKind` to its stable `FCT-NNN` ID, protocol
    class, and retention class. Add a verifier/test that extracts the normative Markdown catalog
    and proves exact bidirectional coverage with no duplicate or invented family.
  - Implement deterministic `hq-testkit` byte/ID/key streams, explicit clock, semantic-fact
    builder, DAG builder, arrival permutations, and small state-machine command sequence builder.
    All generators take an explicit seed/state, produce shrink-friendly ordered data, and expose no
    global random or clock source. Update the walking skeleton to use a catalog payload fixture.
  - Add tests first for catalog coverage, scope/payload matching, required constructor validation,
    deterministic replay and fork behavior, graph parent construction, arrival permutations, and
    enough fixtures to instantiate every acceptance-scenario domain. Document the payload/testkit
    contract and run all Rust, architecture, dependency, target-matrix, whitespace, and unchanged
    Go gates before archiving the package.

  Risks and decisions:

  - The domain catalog records semantic fields and invariants but does not assign JSON keys, enum
    tags, numeric Nostr kinds, signature bytes, SQL columns, or local API shapes.
  - A shared payload record is used only when fields and intrinsic validation are truly identical;
    distinct `FactKind` and `SemanticPayload` variants preserve exhaustive reducer matching.
  - Remote-control payloads live in the same semantic vocabulary but carry a distinct protocol
    class and cannot be mistaken for canonical project-state facts.
  - Testkit output is deterministic test data, not production identity, entropy, cryptography, or
    a promise that generated graphs are semantically usable before reducer validation.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-27 — Causal graph and complete-batch reducer framework

Replaced the reducer summary skeleton with an immutable deduplicating `FactSet`, absorbing identity
collisions, complete parent/reverse indexes including missing vertices, iterative structural and
usable reachability, exact cycle membership, reverse-dependant closure, deterministic dependency
order, and exact usable aggregate frontiers. Added fixed-point normalized decisions with separate
missing and present-unusable blockers, a pure generic domain-reducer seam, transitive projection
support, normalized conflict observations, and the exact typed Kahn presentation comparator. The
application/node walking path now consumes the graph-only complete report. Twelve causal-law and
adversarial tests cover all 64 four-node generated DAGs, merge/permutation/duplicate invariance,
clock reversal, failure propagation, collisions, cycles, every decision class, support, and
non-convergent policy rejection. Workspace format/check/build/test/doctest/strict-Clippy gates,
architecture/spec verifiers, cargo-deny, all four core targets, Go build/vet, and a fresh full Go
test run pass.

### Original plan entry

- **[algebra/high] Implement the causal graph and complete batch reducer framework** — Implement
  immutable fact-set ingestion, deduplication, parent and reverse-dependency graphs, reachability,
  topological processing, unresolved dependency tracking, causal frontiers, projection support,
  normalized reduction decisions, and the single canonical presentation comparator. Expose one pure
  complete-batch reduction entry point and no storage/runtime dependency. Use generated DAGs to
  prove merge semilattice laws, permutation and duplicate invariance, parent-before-child ordering,
  deferred readiness, and exact maximal frontiers. Complete this work when domain reducers can plug
  into a lawful batch engine and no arrival or receiver clock affects semantic output.

  **Implementation plan**

  - Add failing public-API and generated-DAG tests first for exact deduplication, unequal-content ID
    collisions, missing and unusable blockers, present cycles, reverse dependencies, structural and
    usable reachability, exact aggregate frontiers, and deterministic parent-before-child order.
  - Replace the reducer walking skeleton with small pure modules for immutable fact-set ingestion,
    graph indexes, normalized decisions/reasons, domain-stage integration, projection support, and
    presentation ordering. Use ordered collections for normalized output and iterative graph
    algorithms so input iteration order and recursion depth cannot affect results.
  - Define a decoupled domain-stage interface that receives explicit complete-set/graph context and
    returns only closed semantic decisions and typed projection contributions. Provide a permissive
    stage for graph-law tests while preserving a single complete-batch entry point for later
    authority, conversation, agent, and project reducers.
  - Compute readiness and usability to a deterministic fixed point: collisions and cycles fail
    intrinsically, absent parents are listed separately from present-unusable parents, and no
    unusable fact carries causal support. Derive exact usable frontiers and transitive support only
    after decisions stabilize.
  - Implement the reducer-owned Kahn presentation comparator using explicit typed presentation
    keys, retaining causal precedence even when signed clocks move backwards and returning an
    explicit invalid-order error for cyclic selected input.
  - Prove `LAW-MERGE-SET-UNION`, `LAW-INPUT-INVARIANCE`, `LAW-CAUSAL-DOMINANCE`,
    `LAW-EXACT-MAXIMAL-FRONTIERS`, and `LAW-DEFERRED-READINESS` across deterministic generated DAGs,
    arrival permutations, duplicates, and clock-skew cases. Document the public framework and its
    boundary from protocol, storage, runtime, and receiver clocks.
  - Run formatting, workspace check/build/test/doctests, strict Clippy, architecture verification,
    dependency policy, and the retained Go regression suite before recording the package.

  **Risks and mitigations**

  - A one-pass domain callback could make a later conflict or revoke input-order-sensitive; expose
    complete-set context and repeat domain classification to a stable normalized result.
  - A graph-only topological order could accidentally choose a domain winner; keep presentation
    ordering and semantic admission as separate typed operations and never use timestamps or IDs as
    authority/conflict rules.
  - Collision and cycle members cannot safely support descendants; represent both with closed reason
    codes and propagate them as present-unusable blockers rather than silently dropping them.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including the repository-wide gates named
     above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Peer, capability, and human-account reduction

Added exact catalog authority roles and a pure `AuthorityReducer` with explicit local policy,
normalized installation, mailbox, peer-route, capability, account, membership, and selection
projections, exact causal support, and closed decisions. The reducer enforces signer, scope, role,
subject, audience, unique-root, remove-wins, owner-observation, historical-authority, and full-frontier
regrant/reaccept rules. Added causal fixtures and executable coverage for `AUTH-001` through
`AUTH-022`, including all 5,040 arrival orders for the mailbox race. Workspace format, architecture
and spec verification, check/build/test/doctest/strict-Clippy, cargo-deny, four-target core checks,
whitespace, and the complete Go build/vet/fresh-test regression suite pass.

### Original plan entry

- **[authority/high] Implement peer, capability, and human-account reduction** — Add pure reducers
  for installation-local identity/binding facts, directional peer routing, mailbox access grants,
  observations and revokes, human-account creation, device grants, acceptances, revocations,
  selection, and membership frontiers. Authorization must use explicitly cited historical facts at
  the action's causal point. Prove that observed pre-revoke actions survive, concurrent or later
  unauthorized traffic fails closed, and a regranted device becomes authoritative only through a
  causal-maximal acceptance descending from the revoke. Cover missing authority, conflicting roots,
  every topological arrival order, and unrelated-parent attacks. Complete this work when the full
  authority race matrix and batch-reduction laws pass.

  **Implementation plan**

  - Add failing authority fixtures and public-contract tests first for installation and mailbox
    roots, exact local signer/scope rules, peer route block/restore frontiers, directional mailbox
    grants, owner observations, grant revocation, human-account roots, device grant/accept/revoke,
    local account selection, and authorization of later peer/account-scoped fact families.
  - Introduce typed authority aggregate keys, closed rejection/conflict reasons, and normalized
    projections for installations, mailboxes, peer routes, capability lineages, human accounts,
    device memberships, and local selections. Every active projection will retain exact support and
    every multivalue or unique-root conflict will expose all participants.
  - Validate required parent kinds, typed authority roles, signer, subject, audience, and scope
    independently; never infer authority from an ordinary ancestor, peer route, current display
    state, relay metadata, or a fact ID/timestamp ordering. Treat wrong signer/subject relationships
    as invalid and available-but-insufficient historical authority as unauthorized.
  - Derive peer routing as a remove-wins register: concurrent block beats route set, a restore must
    descend from every maximal block, and unrelated descendants cannot clear a block. Keep route
    history visible while emitting no routable singleton for a conflicted or blocked frontier.
  - Derive mailbox capability history at each action's causal point. Require actions to cite the
    exact matching grant, preserve only actions made usable before an owner-signed observation that
    a revoke cites, reject concurrent/post-revoke old-grant actions, and require a regrant lineage
    to descend from every maximal prior revoke.
  - Derive human membership from one account creator root plus exact target-key acceptance. Apply
    remove-wins across every causal-maximal acceptance/revoke, require post-revoke grant and
    acceptance lineages to descend from all revoke maxima, and accept account-scoped actions only
    through the creator or one active maximal acceptance for the exact account.
  - Exhaust every topological arrival order for the named `AUTH-001` through `AUTH-022` race shapes,
    plus duplicates, conflicting roots, missing parents, partial frontiers, changed payload/key,
    wrong account/direction, and unrelated-parent attacks. Re-run all reducer laws to prove the
    authority stage preserves complete-batch input invariance and projection retraction.
  - Document the public authority model and run format, workspace check/build/test/doctests, strict
    Clippy, architecture/spec verification, dependency policy, four-target core checks, whitespace,
    and the unchanged Go build/vet/fresh full regression suite before recording the package.

  **Risks and mitigations**

  - Revocation is non-monotone in projections even though knowledge is add-only; compute authority
    from the complete usable graph on every batch and include revokes/observations in aggregate
    membership so fixed-point reclassification retracts affected descendants.
  - A historical acceptance can remain structurally maximal in a partial lineage; require the exact
    grant/accept payload and signer match and compare against every maximal revoke before granting
    account authority.
  - Base authority must remain reusable by conversation, activity, project, and remote-control
    reducers; keep policy in a focused pure module with typed normalized outputs and no dependency
    on those packages' projection rules.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named authority scenario
     and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.


## 2026-08-27 — Conversation and activity reduction

Added a pure authority-composed `ConversationReducer` with normalized question/async threads,
independent answer/cancellation relation matrices, stable message-ID conflicts, remove-wins
archive/restore, absorbing rejection, causal peer-receipt evidence, typed action groups and final
answers, and inert incomplete addressed observations. Completed the typed activity payload and
implemented source/provider/session/operation/item/runtime namespaces, semantic-sequence winners,
explicit sequence/runtime conflicts, durable completed history, and deterministic newest-200 progress
retention. All `CONV-001`–`CONV-016`, `ACT-001`–`ACT-009`, and `REG-002` cases execute,
including exhaustive small permutations and a 205-record rebuild. All Rust workspace, strict-Clippy,
architecture/spec, cargo-deny, four-target core, whitespace, and Go regression gates pass.

### Original plan entry

- **[conversation/high] Implement conversation and activity reduction** — Add questions, answers,
  asynchronous messages, cancellation, archive/restore/reject, delivery-relevant semantic state,
  typed presentation/correlation, and the separate non-actionable harness-activity stream. Define
  one reducer-owned causal ordering comparator and deterministic activity coalescing/retention rules;
  no store or UI may recreate them. Test missing parents, concurrent answer/cancellation, equal-time
  messages and activity, delayed occurrence data, final-answer selection, action grouping, and
  projection retraction. Complete this work when normalized conversation and activity views are
  deterministic for all generated arrival orders.

  **Implementation plan**

  - Add failing public-contract fixtures and tests first for every named `CONV-001` through
    `CONV-016`, `ACT-001` through `ACT-009`, and `REG-002` scenario. Generate small answer,
    cancellation, message-state, and activity graphs across every topological arrival order,
    duplicates, late parents, equal authored/occurrence times, clock reversal, and projection
    retraction.
  - Complete the typed harness-neutral activity value model needed by the retained contract:
    provider/session/operation plus optional item correlation, runtime-lifetime identity, signed
    occurrence time, positive source sequence, kind/status, bounded content, and explicit
    truncation. Keep message purpose, presentation kind, public ID, and operation grouping typed;
    prove that prose imitating authority, correlation, or final-answer markers is inert.
  - Add a focused pure conversation/activity reducer that composes the existing authority policy
    without duplicating its rules. Introduce closed reasons, typed aggregate/projection keys, exact
    support, incomplete addressed observations, and normalized thread, message-state, delivery,
    action-group, final-answer, activity-history, activity-winner, collision, and unified-entry
    views.
  - Validate exact root/child/state target kinds, derived thread identity, sender/recipient reversal,
    compatible scope and correlation, required causal ancestry, controlling mailbox/account, and
    complete state frontiers. Treat unequal stable message IDs as explicit conflicts; retain answers
    and cancellations independently and expose every before/after/concurrent relation.
  - Implement archive/restore as a remove-wins register over causal maxima and rejection as an
    absorbing tombstone. A restore opens only after every maximal archive and never after rejection;
    state facts remain auditable while open/action projections retract and exact frontier/support
    changes are visible.
  - Derive peer-received evidence only from a usable peer-authored child that cites the outbound
    message, never from relay or receipt metadata. Select ready answers and typed final answers only
    through the reducer-owned canonical presentation traversal, retaining all candidates and using
    operation correlation as the action group.
  - Keep activity in a disjoint non-actionable stream. Coalesce snapshot/progress facts only within
    the full source mailbox, provider, session, operation, kind, item, and runtime namespace; choose
    higher semantic sequence across concurrent snapshots, report equal-sequence unequal-content or
    conflicting-runtime collisions, retain completed items as history, and deterministically keep
    the newest 200 progress winners per source/provider session while canonical facts remain intact.
  - Feed all projected messages and retained activity through the sole typed Kahn comparator,
    including occurrence and correlation tie breakers while preserving parent-before-child order.
    Document the normalized model and run format, architecture/behavior/spec verifiers, workspace
    format/check/build/test/doctests, strict Clippy, dependency policy, four-target core checks,
    whitespace, and unchanged Go build/vet/fresh full regression suite before recording the package.

  **Risks and mitigations**

  - Conversation classification depends on historical authority but must also expose domain-specific
    decisions and projections; factor reusable authority-stage helpers and wrap their closed reasons
    rather than copying or weakening signer, scope, membership, and revocation checks.
  - Stable presentation order and activity winner selection solve different problems; use semantic
    sequence only inside an exact activity aggregate, then order retained entries with the canonical
    comparator so timestamps or fact IDs never decide a conflict.
  - Incomplete addressed content is intentionally observable before it is usable; expose it through
    a separate inert projection that cannot support a thread, action, delivery, archive, answer, or
    final-answer view until the missing causal history projects.
  - Activity retention is a disposable-view policy over permanent canonical facts; compute the
    budget from complete normalized winners with a total typed key so batch, late-parent replay, and
    repair rebuilds select exactly the same 200 rows.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named conversation and
     activity scenario and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Named-agent and provider-session reduction

Implemented a pure, authority-composed named-agent reducer with permanent name reservations,
immutable provider-session bindings, repository-context history, causal selection and rename
registers, absorbing retirement, and projectless direct-session projections. Added executable
coverage for every `AGT-001` through `AGT-010` scenario, including permutation and duplicate replay,
and documented the public model. The complete Rust workspace, architecture/behavior/spec verifiers,
strict Clippy, dependency policy, all four core targets, whitespace checks, and unchanged Go
build/vet/fresh regression suite pass.

### Original plan entry

- **[agents/high] Implement named-agent and provider-session reduction** — Add pure facts and
  projections for mailbox creation/binding/context, permanent name claims and retirement, durable
  provider-session bindings, selection, renaming, repository context, and projectless direct
  sessions. Keep durable session identity separate from runtime presence, leases, caller
  environments, and process state. Define name/session conflicts and replay behavior explicitly and
  test rebuildable history, retirement, reselection, and cross-provider namespace isolation.
  Complete this work when all retained named-agent state derives solely from the fact set.

  **Implementation plan**

  - Add failing fixtures and public-contract tests first for every named `AGT-001` through
    `AGT-010` scenario, then generate small claim/binding/context/selection/rename/retirement graphs
    across every arrival order, duplicates, late parents, partial frontiers, and clock reversal.
  - Add a pure named-agent reducer that composes historical authority without duplicating it and
    emits closed reasons, typed aggregate/projection keys, exact support, permanent name
    reservations, mailbox/session histories, context frontiers, selection/rename registers,
    retirement state, and projectless direct-session views.
  - Validate installation-local signer/scope, exact agent mailbox roots, lowercase name syntax,
    stable agent/name/mailbox subjects, typed claim and binding parents, provider/session namespace,
    selected immutable repository context, and complete selection/rename frontiers independently.
    Repository context remains display/search metadata and grants no authority.
  - Treat one name, agent ID, or agent mailbox claimed incompatibly as an explicit permanent
    conflict. Keep a retired name reserved forever, expose every participant, and never use authored
    time, fact ID, arrival, or current runtime state to select a claimant.
  - Treat provider plus session as one immutable binding identity: rebinding it to another mailbox
    conflicts, one mailbox may retain several distinct sessions, and equal session text in different
    providers remains isolated. Retain unnamed projectless mailbox/session history without
    inventing a named runnable agent.
  - Derive selection as a multivalue causal register. Concurrent distinct selections expose every
    maximum and block runnable selection; one later selection resolves only when it descends from
    every prior maximum and cites the exact name claim, binding, and matching repository context.
  - Derive per-session display rename as an independent multivalue register with sorted candidates,
    explicit clear, exact frontier/support, and no effect on selection or runtime. Retain all
    mailbox context history and every causal-maximal context value.
  - Make retirement absorbing and remove-wins against concurrent selection/rename state. Historical
    sessions, names, contexts, and selections remain queryable, but no retired/conflicted agent is
    runnable and no post-retirement session fact can reactivate it. Prove normalized output contains
    no process, lease, presence, phase, caller environment, or ambient filesystem state.
  - Document the public named-agent/session model and run format, architecture/behavior/spec
    verifiers, workspace format/check/build/test/doctests, strict Clippy, dependency policy,
    four-target core checks, whitespace, and unchanged Go build/vet/fresh full regression suite
    before recording the package.

  **Risks and mitigations**

  - Authority already admits installation-local binding families but does not own their global
    uniqueness or agent semantics; reuse its classification as the first stage, then apply focused
    name/session rules without altering prior authority projections.
  - Selection facts embed repository context while context facts are grow-only and may be
    concurrent; require an exact projected mailbox-context value cited in the selection lineage and
    expose context ambiguity instead of choosing a timestamp winner.
  - Retirement is non-monotone only in runnable projections; recompute from the complete usable
    history so late retirement retracts active selection while immutable facts and permanent name
    reservation remain.
  - Direct unnamed sessions and named managed agents share binding history but not lifecycle; keep
    distinct typed projections so a mere binding never synthesizes a claim, selection, or runnable
    worker.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named agent/session
     scenario and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Pure project and resource-claim model

Implemented the pure home-linear project reducer, explicit lifecycle/archive/resource transition
model, pluggable path-claim policy, global assignment cardinality, exact agent/session and thread
binding, contiguous input sequencing, at-most-once dispatch attribution, current/late output
classification, and isolated remote-command stages. Added grouped executable coverage mapping
every PRJ-001 through PRJ-023 and CTL-001 through CTL-004 scenario and documented the public
model. The complete Rust workspace, architecture/behavior/spec verifiers, strict Clippy,
dependency policy, all four core targets, whitespace checks, and unchanged Go build/vet/fresh
regression suite pass.

### Original plan entry

- **[projects/high] Implement the pure project and resource-claim model** — Add project identity and
  immutable home, mailbox, metadata, predecessor, desired resources, primary path, lifecycle,
  archive state, active claims, assignment epochs, thread scope, project input sequencing, dispatch
  attribution, expected-head compare-and-swap, remote command/result state, and late-output
  classification. Model reversible domain transitions separately from operational saga states and
  keep resource-kind policy behind explicit pure interfaces. Test stale heads, concurrent commands,
  assignment cardinality, close/reopen/archive laws, force-takeover authority, and inactive-output
  behavior. Complete this work when the project transition model satisfies every invariant in the
  retained project specification without filesystem or provider I/O.

  **Implementation plan**

  - Add failing public-contract fixtures first for every named `PRJ-001` through `PRJ-023` and
    `CTL-001` through `CTL-004` acceptance scenario. Generate small home-linear histories and
    global project sets across arrival permutations, duplicates, late parents, partial frontiers,
    stale siblings, and authored-clock reversal.
  - Add a pure project reducer that composes historical authority plus named-agent and conversation
    projections without copying their rules. Emit closed decisions, typed aggregate/projection
    keys, exact support and blockers, immutable project identity, unique home/mailbox roots,
    complete accepted history, authoritative head/frontier, and explicit fork participants.
  - Express the home transition algebra as pure functions over typed state. Creation establishes
    immutable home, mailbox, predecessor, metadata, desired resources, optional primary path,
    lifecycle, archive state, and input sequence; every later canonical project fact must cite the
    exact unique head, and sibling or stale children remain visible without becoming a winner.
  - Separate stable lifecycle from operational preparation/closing/configuring states. Enforce
    atomic reopen, resource replacement, activation compensation, close, force-close, reopen,
    archive, and unarchive laws; archive requires closed and unassigned state, unarchive yields a
    visible closed claim-free project, and runtime observations never assert external cessation.
  - Put resource overlap behind a pure policy interface. For first-release path resources compare
    home-qualified canonical locators for equality or ancestor/descendant overlap, permit overlap
    within one project and equal spelling across homes, activate all desired claims atomically, and
    expose every cross-project conflict without using fact ID, timestamp, or arrival as a winner.
  - Model assignment epochs explicitly from configuring through runnable, blocked, and ended.
    Enforce at most one active agent per project and one active project per agent, exact selected
    immutable project-thread scope, provider-session binding, launch context, graceful/forced end
    authority, conflict retraction, and restoration when a competing epoch validly ends.
  - Derive one contiguous home input sequence and immutable at-most-once dispatch attribution.
    Validate the exact accepted project message, current runnable assignment, agent, scoped thread,
    provider session, and sequence; expose duplicate ID/sequence/dispatch conflicts rather than
    choosing a branch.
  - Retain project output by stable ID and complete provenance. Deduplicate identical retries,
    conflict any changed body/presentation/correlation/binding, classify output against the complete
    assignment history as current or late-from-inactive, and never allow output to mutate lifecycle,
    claims, assignment, or dispatch authority.
  - Derive remote command views independently from canonical project state: active-device requests
    queue only, home receipts record the observed head, committed outcomes cite canonical descendant
    facts, rejected outcomes retain typed stale/current-head and runtime certainty, and unequal
    receipt or terminal values conflict without a selected result.
  - Document the public project/resource model and run format, architecture/behavior/spec
    verifiers, workspace format/check/build/test/doctests, strict Clippy, dependency policy,
    four-target core checks, whitespace, and unchanged Go build/vet/fresh full regression suite
    before recording the package.

  **Risks and mitigations**

  - Home-linear validity and global safety can retract different projections; compute accepted
    history first, then derive project state, path conflicts, assignment cardinality, dispatch, and
    output status as separate deterministic passes with explicit cross-pass inputs.
  - A generic workflow framework would obscure project-specific transition laws; keep a small typed
    transition function and explicit per-fact validation, sharing only proven graph/frontier and
    multivalue helpers.
  - Resource identity in this package is semantic rather than filesystem-derived; accept only typed
    canonical locators and delegate materialization, symlink revalidation, health, and release
    assessment to later adapters while proving those observations cannot change lifecycle.
  - Remote control and operational saga checkpoints are observable but not competing project-state
    authorities; project only home-signed canonical facts and retain command/runtime uncertainty in
    disjoint views.
  - The payload catalog is intentionally large; keep normalized state and view structs bounded and
    keyed, factor validation by semantic family, and use focused generated fixtures to prevent one
    monolithic reducer path from hiding invariant gaps.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every named project/control
     scenario and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, and plan bookkeeping form one reviewable change.

## 2026-08-27 — Canonical fact, remote-control, and trust specifications

Specified independent `hq/canonical` v1 and `hq/control` v1 protocols with strict canonical UTF-8
JSON, named raw/decoded/encoded bounds, signed scope and typed cross-namespace causal references,
exact NIP-01 event construction, and provisional regular kind 6000 selected through a revision-pinned
ADR. Published an exhaustive owned DTO mapping for all 48 semantic families and an explicit trust
state/failure model that prevents malformed, failed, or verified-unsupported input from exposing a
semantic fact. Added exact canonical and control vectors whose preimages reproduce their SHA-256 IDs
and whose BIP-340 signatures passed both `nak 0.20.2` and the independent btcsuite Schnorr verifier,
plus a machine-readable adversarial corpus and consistency/link/vector-integrity checks. All Rust
workspace, strict-Clippy, architecture/behavior/causal/protocol-spec, cargo-deny, four-target core,
whitespace, and unchanged Go build/vet/fresh regression gates pass.

### Original plan entry

- **[protocol/high] Specify canonical facts, remote control, and trust transitions** — Write
  canonical fact v1 and remote-control v1 as new protocols with independent version spaces,
  deterministic encoding rules, strict decoding policy, size/count/text bounds, event identity,
  signatures, audience and authority representation, unsupported-version behavior, and exact trust
  transitions from raw bytes to verified semantic facts. Decide the provisional Nostr application
  kind and encoding using an ADR rather than inheriting Go values. Define exact vectors and
  adversarial cases before implementation. Complete this work when every semantic fact has an
  unambiguous DTO mapping and no domain struct accidentally serves as a wire schema.

  **Implementation plan**

  - Verify the current primary NIP-01/NIP registry requirements for event serialization, identity,
    Schnorr signatures, application-kind ranges, and extensibility. Record the checked revisions
    and write an ADR selecting one provisional immutable application kind plus its compatibility
    and registration posture without inheriting any Go kind or schema.
  - Specify two independent versioned namespaces: canonical fact v1 for `FCT-001` through
    `FCT-045`, and remote-control v1 for `FCT-046` through `FCT-048`. Give each an explicit media
    shape, protocol discriminator, version field, supported-family registry, typed ID namespace,
    and unsupported-version/family retention behavior.
  - Define the exact UTF-8 JSON wire grammar and one canonical byte form: object member order,
    integers, booleans, null/omission, string escaping, Unicode policy, arrays, duplicate and unknown
    members, trailing data, depth/count limits, and rejection of semantically equal non-canonical
    spellings. Bounds apply to decoded semantic values and to final encoded bytes after escaping.
  - Define NIP-01 event construction independently from the payload DTO: exact application kind,
    fixed tag vocabulary/order, empty-versus-present tag rules, content bytes, event serialization,
    SHA-256 identity, 32-byte lowercase hex, BIP-340 signing, signature verification, public-key
    agreement, and preservation of the exact verified event and content bytes.
  - Specify signed scope/audience DTOs and typed causal references. Encode parents as a sorted unique
    list and authorities as sorted unique role/fact pairs whose IDs also occur in parents; define
    canonical and remote-control cross-namespace reference rules and reject unknown roles, duplicate
    roles, role/parent mismatch, and audience/author contradictions before semantic construction.
  - Publish an exhaustive mapping table from every semantic payload field and nested enum/value to
    an owned protocol DTO field, including numeric catalog family IDs, bounded text/collections,
    optional values, timestamps, nonzero sequences, repository/resource locators, messages,
    activity, agent/session, project/assignment/input/output, and remote command/result/runtime
    variants. Domain enum or Rust field spelling is never normative wire vocabulary.
  - Define the trust-state machine and failure taxonomy from untrusted raw event bytes through
    bounded outer parse, canonical event verification, exact content retention, protocol dispatch,
    bounded DTO parse, canonical re-encoding equality, semantic conversion, and reducer admission.
    Raw, parsed, cryptographically verified, verified-supported, verified-unsupported, and semantic
    values remain distinct and no failed or unsupported state exposes a `SemanticFact`.
  - Add exact hand-checkable canonical and remote-control vectors with payload bytes, NIP-01 event
    preimage, event ID, public key, signature, and expected semantic mapping, plus adversarial corpora
    for malformed JSON, escaping, duplicate/unknown fields, ordering, bounds, invalid hex,
    wrong kind/version/family, namespace confusion, tampering, bad signatures, and authority/scope
    mismatch. State which independent implementation or standard vector validates crypto values.
  - Add machine-readable protocol-spec consistency tests that prove all 48 catalog families appear
    exactly once in the mapping/registry, protocol ranges remain disjoint, every bound is named,
    vectors are exact files rather than prose ellipses, and ADR/spec links are complete.
  - Run documentation format/link checks, architecture/behavior/causal-spec verifiers, workspace
    format/check/build/test/doctests, strict Clippy, dependency policy, four-target core checks,
    whitespace, and unchanged Go build/vet/fresh full regression suite before recording the package.

  **Risks and mitigations**

  - Nostr kind registration and NIP text can change; cite the exact upstream revision reviewed,
    select a provisional regular-event kind through the ADR, and isolate the kind as outer carriage
    so a future registered value does not silently change canonical payload v1 bytes.
  - Generic JSON libraries accept multiple spellings and usually lose duplicate/order information;
    specify validation over exact retained bytes and canonical re-encoding equality before choosing
    an implementation library in the next package.
  - One shared version field could couple immutable facts to remote workflow evolution; keep two
    discriminators and version registries even though both ride the same signed-event boundary.
  - Exhaustive payload mapping is large and typo-prone; key it to stable `FCT-xxx` numbers, generate
    consistency assertions from the domain catalog, and require a named DTO for every nested sum
    type instead of an untyped payload map.
  - A cryptographically valid event is not necessarily an authorized or meaningful HQ fact; make
    each trust transition explicit and preserve verified unsupported input without allowing it into
    semantic conversion or reduction.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the specification in proportion to risk, including registry/mapping consistency,
     exact vectors, adversarial cases, and every repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so specifications, tests, and plan bookkeeping form one reviewable
     change.

## 2026-08-27 — Signed-event cryptographic trust boundary

Implemented bounded exact HQ NIP-01 event parsing and encoding, SHA-256 event identity, raw
32-byte BIP-340 signing and verification, retained raw/preimage/content bytes, and disjoint
raw, parsed, cryptographically verified, supported, and verified-unsupported owners. Added a
specialized canonical JSON cursor and closed redacted failures for malformed wire shapes, limits,
wrong kind/tags, tampering, invalid keys/signatures, authored-time disagreement, namespace
confusion, unsupported prefixes, and frozen Go schemas. Published tests for both signed-event
vectors and selected official BIP-340 valid/invalid vectors, a compile-fail trust-state proof,
adversarial boundary coverage, and a seeded raw-byte cargo-fuzz smoke gate pinned to cargo-fuzz
0.12.0 and nightly-2026-08-26. `k256` 0.14 and `sha2` 0.11 remain pure Rust and compile on all
four release triples; root and isolated-fuzz dependency policies, every Rust workspace/spec/
architecture gate, whitespace, and unchanged Go build/vet/fresh regression suite pass.

### Original plan entry

- **[protocol/high] Implement strict signed-event framing and the cryptographic trust boundary** —
  Implement bounded exact NIP-01 outer-event parsing/encoding, SHA-256 identity, BIP-340
  signing/verification, retained raw/preimage/content bytes, distinct raw/parsed/verified types, and
  bounded protocol-prefix dispatch into supported content or verified-unsupported records. Reject
  wrong kind/tags, non-canonical outer JSON, tampering, bad keys/signatures, time disagreement, and
  old Go schemas before DTO or reducer access. Complete this split package when the published event
  vectors and independent BIP-340 vectors pass and no unverified value can call a verified API.

  **Implementation plan**

  - Add failing public API tests first for the two published signed-event vectors, selected official
    BIP-340 valid/invalid vectors, exact byte retention, each trust-state constructor boundary,
    deterministic preimage/ID reconstruction, and an explicit signer supplied auxiliary randomness.
  - Add narrowly owned protocol dependencies at current reviewed releases: pure-Rust `k256` Schnorr
    verification/signing and `sha2` hashing, with default features minimized, workspace dependency
    policy updated, licenses audited, and four-target compilation retained. Keep key material out of
    domain types and ensure signer/debug/error surfaces never expose a secret.
  - Replace the walking-skeleton-only boundary with immutable `RawEventBytes`, `ParsedOuterEvent`,
    `CryptographicallyVerifiedEvent`, `SupportedContentBytes`, and
    `VerifiedUnsupportedRecord` owners. Preserve the walking skeleton only as an explicitly
    non-normative compatibility path until later application work removes it.
  - Implement a specialized allocation-bounded JSON cursor for the exact seven-member outer object
    and NIP-01 string escaping. Enforce member order, unknown/duplicate/missing rejection, minimal
    integer and escape spellings, valid UTF-8/scalars, empty tags, no whitespace/trailing bytes, and
    raw/content limits before copying attacker-controlled data.
  - Encode the exact event-ID preimage and outer event without a generic JSON value. Recompute
    SHA-256 before signature verification, compare IDs without data-dependent early exit, parse
    x-only keys/signatures canonically, verify BIP-340 over the 32-byte event ID, and retain exact
    raw, reconstructed preimage, and decoded content bytes.
  - Add a signer boundary that accepts a validated secret-key owner plus caller-supplied
    cryptographic auxiliary randomness, derives the x-only public key, signs the precomputed event
    ID exactly once, self-verifies, zeroizes key material through its crypto owner, and produces the
    same immutable verified representation used by inbound events.
  - Implement bounded prefix dispatch for the exact ordered `p`, `v`, and `f` content fields.
    Distinguish supported canonical/control content, verified unsupported protocol/version/family,
    namespace confusion, and malformed prefixes without constructing payload DTOs or semantic facts.
  - Add boundary and adversarial tests for zero/maximum lengths, one-byte-over limits, malformed
    UTF-8/JSON/escapes/hex/integers, reordered/duplicate/unknown members, nonempty tags, wrong kind,
    ID/content/signature tampering, invalid curve points and signature scalars, namespace confusion,
    unsupported values, and legacy Go event/schema samples. Prove failure values expose no verified
    content and unsupported values expose no supported content.
  - Add a raw-byte cargo-fuzz target and seeded corpus for outer parse/verify/dispatch; build it in a
    pinned short smoke gate and document longer sanitizer runs. Run format, all spec/architecture
    verifiers, workspace check/build/test/doctests, strict Clippy, dependency policy, four-target
    core checks, whitespace, and unchanged Go build/vet/fresh regression suite before recording.

  **Risks and mitigations**

  - A prehash/signing-trait mismatch could silently sign SHA-256(event-ID) instead of the NIP-01
    event ID; use the explicit prehash interfaces, official BIP-340 vectors, published HQ vectors,
    and independent verifier results to pin the exact 32-byte message semantics.
  - Generic JSON parsing can normalize duplicates, ordering, numbers, or escapes before policy sees
    them; keep the small outer grammar in a byte cursor and require deterministic re-encoding where
    decoded strings are involved.
  - Unsupported content must be retained only after cryptographic proof but must not be mistaken for
    a valid DTO; use disjoint types with no shared semantic conversion method and classify only a
    bounded canonical prefix.
  - Signing APIs can accidentally make secrets cloneable or printable; wrap the zeroizing crypto
    key, omit `Clone`/`Debug`/serialization, take explicit auxiliary randomness, and return closed
    redacted errors.
  - Fuzz tooling uses nightly and may not exist on a contributor machine; keep deterministic corpus
    regression tests on stable, pin cargo-fuzz for CI/smoke use, and treat longer fuzz duration as an
    additive security gate rather than replacing ordinary tests.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including every vector, boundary,
     adversarial case, fuzz smoke, and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so code, tests, fuzz assets, and plan bookkeeping form one reviewable
     change.

## 2026-08-27 — Canonical v1 owned DTO catalog

Implemented the complete owned `hq/canonical` and `hq/control` v1 DTO catalog for all 48 numeric
families, including fixed lowercase-hex values, required nullable fields, exact tagged scopes and
references, every nested object/sum type, named decoded bounds, positive sequences/timestamps,
family scope and authority-role applicability, sorted unique parents/authorities, authority-parent
linkage, and core representational invariants. Full decoding prevalidates bounded canonical JSON,
uses typed Serde 1.0.229 DTOs plus only `serde_json::RawValue` body isolation, rejects unknown/
duplicate/missing fields, deterministically re-encodes with serde_json 1.0.151, and advances to
`VerifiedSupportedRecord` only after byte equality with retained signed content. Added executable
exact round trips for every family and both published vectors, adversarial nested/ordering/
namespace/enum/hex/boundary coverage, inclusive named text-bound tests, and a second re-signing
cargo-fuzz target seeded by canonical and control contents. Both fuzz targets, root and isolated
dependency policies, all four protocol targets, every Rust workspace/spec/architecture gate,
whitespace, and unchanged Go build/vet/fresh regressions pass.

### Original plan entry

- **[protocol/high] Implement strict canonical v1 DTO decoding and encoding** — Implement the
  complete owned canonical/control v1 DTO catalog, strict full-content decoding, deterministic
  encoding, duplicate/unknown/missing/reordered-field policy, enum and fixed-width primitive
  vocabulary, decoded and post-escaping bounds, sorted reference representation, and a distinct
  verified-supported DTO trust state. Add exhaustive round-trip fixtures for all 48 families,
  independent exact vectors, malformed/non-canonical/boundary corpora, and structure-aware fuzzing.
  Complete this split package when every normative DTO shape is executable and no non-canonical or
  merely prefix-supported input can acquire a fully verified DTO.

  **Implementation plan**

  - Add failing public-contract tests first for the two published vector contents and a table with
    one complete valid content record for every family 1 through 48. Assert exact family/namespace
    dispatch, owned DTO variant selection, byte-for-byte re-encoding, exact verified-event
    retention, and a disjoint `VerifiedSupportedRecord` state reached only from
    `SupportedContentBytes`.
  - Add current reviewed `serde` 1.0.229 with derive and `serde_json` 1.0.151 with only the required
    standard/raw-value features to `hq-protocol`. Use them only for statically typed owned DTOs and
    raw body isolation; never construct or expose `serde_json::Value`, maps, or domain serialization.
    Audit the expanded graph and retain compilation on all four release targets.
  - Define one exact common content envelope and owned protocol types for fixed hex, required
    nullable properties, scope arrays, namespace-qualified parents, role-qualified authorities,
    locators, contexts, operations, messages, resources, bindings, activity/runtime/result sums,
    and every family body. Keep wire names and enum spellings in `hq-protocol`, independent of Rust
    domain field names.
  - Reuse the bounded canonical JSON prevalidator before Serde allocation. Deserialize the common
    envelope with `deny_unknown_fields` and a retained raw body, require all nine properties,
    cross-check discriminator/version/family against the consumed prefix state, then deserialize
    the body into the exact family DTO. Reject duplicate, missing, unknown, wrong-type, overflow,
    invalid UTF-8/scalar, floating-point, negative, and trailing input before producing a verified
    DTO.
  - Validate decoded primitives and collection representation without semantic construction:
    lowercase 32-byte hex, nonempty named text bounds, positive sequences, signed-millisecond
    range, closed enum spellings, object/array limits, unique relay/resource identities, sorted
    unique namespace-qualified parents, sorted unique authority triples, authority-as-parent, legal
    canonical/control reference directions, and family-applicable authority-role vocabulary.
  - Serialize only from owned DTO structs in normative member order with every optional property
    present as a value or `null`. Enforce final content bounds after escaping and require the result
    to equal the retained verified content bytes before constructing `VerifiedSupportedRecord`;
    classify every semantically equal alternate spelling or member order as non-canonical.
  - Provide a deterministic outbound DTO encoder that performs the same validation and size checks
    before yielding bytes suitable for the existing signer, while retaining the DTO owner for the
    following semantic-conversion package. Keep signing, authorization, and reduction outside the
    DTO module.
  - Add adversarial tests covering every common/nested shape, required-null omission, duplicate and
    unknown properties at each depth, reordered members, nonminimal escapes and numbers, bad hex,
    unknown enums, wrong body/family pairs, invalid reference namespaces/order/roles, zero/overflow
    sequence/time values, duplicate/oversized collections, decoded text limits, and one-byte-over
    final escaped content. Include frozen Go schema samples and prove failures expose no DTO.
  - Extend the isolated cargo-fuzz workspace with a content target seeded by both published vectors.
    Re-sign bounded mutations with the fixture key so parse/dispatch/DTO validation remains
    reachable, run the pinned smoke gate, and document longer sanitizer runs. Run format, all
    spec/architecture verifiers, workspace check/build/test/doctests, strict Clippy, root and fuzz
    dependency policy, four-target protocol checks, whitespace, and unchanged Go build/vet/fresh
    regression suite before recording.

  **Risks and mitigations**

  - Serde normally accepts input spellings and member orders that v1 forbids; treat typed decoding
    as provisional, serialize the exact DTO back into the normative order, and advance trust only
    after byte equality with retained content.
  - `Option<T>` can make a missing property indistinguishable from required `null`; use an owned
    required-nullable wrapper or equivalent visitor so omission is rejected before canonicality
    comparison.
  - A generic body value or untagged enum could normalize duplicates or select an ambiguous shape;
    retain only the raw body slice, dispatch by the already verified numeric family, and deserialize
    exactly one named body type.
  - Forty-eight mappings invite drift and copy/paste gaps; drive the registry from one exhaustive
    numeric match, table-test every family, and add bidirectional consistency assertions against
    `FactKind::ALL` and the normative mapping document.
  - Structure-aware fuzzing cannot mutate a signature-covered payload directly; re-sign only inside
    the isolated harness with a published fixture secret and explicit deterministic auxiliary bytes,
    never add a production bypass around cryptographic verification.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including all 48 family fixtures, every
     malformed/boundary class, fuzz smoke, and repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so DTO code, tests, fuzz assets, and plan bookkeeping form one
     reviewable change.

## 2026-08-27 — Installation identity and local configuration persistence

Implemented the Rust state layout and exclusive process-lifetime ownership boundary in `hq-node`,
with exact private modes, symlink rejection, stable identity/configuration/database paths, durable
same-directory atomic writes, and a fixed bounded binary identity format. Added non-cloneable
zeroizing installation authority, safe public metadata and signer access, canonical typed unsigned
relay/provider defaults, and a versioned encrypted identity-only backup/import package. NIP-49 uses
NFKC passwords, bounded scrypt costs, XChaCha20-Poly1305, classic canonical `ncryptsec` Bech32,
the official vector, authenticated public-key agreement, no-overwrite import/export, and no Go or
database/history/configuration compatibility. Public, adversarial, redaction, compile-fail,
permission, lock, corruption, partial-file, entropy, Unicode, bounds, and round-trip tests pass.
Workspace format/check/build/test/doctests/strict-Clippy, architecture/behavior/causal/protocol
verifiers, root and fuzz dependency policy, four core/protocol targets, whitespace, and unchanged
Go build/vet/fresh full regression gates pass.

### Original plan entry

- **[identity/high] Implement installation identity and local configuration persistence** —
  Implement the new Rust state-directory layout, stable installation identity, root-key generation
  and loading, signer access, secure atomic file creation and permissions, public identity display,
  and the identity export/import/backup behavior retained by the behavior ledger. Keep secret keys
  out of SQLite, logs, diagnostics, RPC results, and canonical facts, and reject unsafe overwrite or
  concurrent-use conditions. Use the ADR-0002 Rust-era encrypted package with NIP-49 secret
  protection, keep database/history migration outside it, and omit a routine recursive reset
  command. Implement typed local configuration for relay and provider defaults without turning
  configuration into signed domain state. Test fresh initialization, partial-write recovery,
  permission failures, redaction, backup round trips, duplicate identity protection, and path
  derivation. Complete this work when the node and store can consume one explicit secure
  identity/configuration boundary without reading Go state or formats.

  **Implementation plan**

  - Add failing public-contract tests first for explicit and environment-derived state paths,
    exclusive node ownership, fresh initialization, reopen with stable identity/public key, signer
    access, public inspection redaction, strict local configuration, encrypted export/import, and
    refusal to overwrite an existing identity. Keep all filesystem tests isolated under unique
    temporary directories and assert the exact Unix directory/file modes on Linux and macOS.
  - Keep the boundary in `hq-node`, the sole composition/I/O owner, rather than adding a new crate or
    leaking filesystem and secret-key concepts inward. Define `StatePaths`, an exclusive
    `StateDirectoryOwner`, a non-cloneable `InstallationIdentity`, redacted closed errors, public
    identity metadata, typed relay/provider configuration, and narrow load/init/export/import APIs
    that later node/store composition can consume without opening each other's files.
  - Specify and implement a fresh fixed binary identity-file v1 containing only a magic/version,
    32-byte installation identity, and valid 32-byte secp256k1 secret. Generate both identities from
    an injectable CSPRNG boundary, reject zero/invalid scalars and malformed/trailing files, retain
    secrets only in zeroizing owners and the crypto signer, and derive public keys rather than
    trusting stored duplicates. Do not parse Go keys, databases, backup JSON, or schemas.
  - Implement the state layout with an explicit root plus stable identity, configuration, database,
    and ownership-lock paths. Derive the default from `XDG_STATE_HOME` or `HOME` without hidden
    database overrides. Create directories as `0700`, identity/config/lock files as `0600`, reject
    symlinks and unsafe existing permissions, acquire the standard-library exclusive file lock, and
    keep its handle alive for the full owner lifetime so concurrent init/load/import fails closed.
  - Centralize durable atomic writes: same-directory unpredictable `create_new` temporary file,
    restricted mode at creation, complete write and file sync, no-overwrite checks for identity and
    backup creation, atomic rename for configuration replacement, parent-directory sync, and scoped
    partial-file cleanup on every ordinary error. Test injected short/write/sync/rename failures or
    an equivalent seam plus recovery in the presence of abandoned unrelated temporary files.
  - Implement NIP-49 exactly over raw secret bytes: NFKC-normalized bounded password, scrypt with
    fixed export cost and encoded/import-validated `log_n`, `r=8`, `p=1`, random 16-byte salt,
    random 24-byte XChaCha20-Poly1305 nonce, one-byte security associated data, version 2, classic
    `ncryptsec` Bech32 encoding, authenticated decryption, and zeroization of password, derived key,
    plaintext, and intermediate secret buffers. Pin the official NIP-49 vector and reject wrong
    passwords, corruption, wrong HRP/checksum/version/length/security byte, and unreasonable KDF
    costs before expensive allocation.
  - Define the surrounding bounded canonical backup package v1 with installation identity, derived
    public key, and `ncryptsec`; export by exclusive durable creation and import only into an absent
    identity while holding state ownership. Strictly reject missing/unknown/duplicate/reordered
    fields and noncanonical encodings, verify decrypted key/public-key agreement, and never include
    history, SQLite, configuration, relay state, provider state, credentials, or operational data.
  - Define canonical unsigned local configuration v1 with a bounded ordered-unique set of typed
    `ws`/`wss` relay endpoints and an optional validated provider default. Load absence as explicit
    defaults, reject unknown/reordered/duplicate/oversized/noncanonical input, atomically replace
    only the configuration file, and expose no conversion from configuration into signed facts.
  - Add adversarial and redaction coverage for malformed/oversized/trailing identity files, invalid
    keys, insecure modes, directory/file symlinks, occupied locks, existing import/export targets,
    bad environment/path inputs, entropy failure, partial-write recovery, backup tampering and
    Unicode normalization, configuration limits, and `Debug`/`Display` surfaces. Run format, all
    architecture/spec verifiers, workspace check/build/test/doctests, strict Clippy, dependency
    policy, four-target core/protocol checks, whitespace, and unchanged Go build/vet/fresh full
    regression suite before recording.

  **Risks and mitigations**

  - A convenient serializable identity type could leak the root secret into logs, RPC, SQLite, or
    canonical facts; keep the secret owner private and non-serializable, implement only redacted
    debug output, and expose public metadata plus signing behavior through separate methods.
  - File modes applied after creation leave a disclosure window and rename can silently overwrite;
    set modes in `OpenOptions` before creation, serialize same-state operations with the owner lock,
    use exclusive creation for identity/backup destinations, and test every collision path.
  - Password encryption is easy to make subtly non-interoperable; pin the normative NIP-49 byte
    layout and official vector, use classic Bech32 rather than Bech32m, normalize NFKC before scrypt,
    authenticate the security byte, and validate cost/length fields before deriving a key.
  - Test-friendly weak KDF parameters can accidentally become a production downgrade; keep the
    production export cost constant, bound imported NIP-49 costs to the reviewed range, and limit any
    cheap deterministic helper to private unit-test code.
  - Crash recovery and durability differ from ordinary successful I/O; use one atomic-write helper,
    sync the completed file and containing directory, clean only temporary paths created by the
    current attempt, and treat an absent final identity as uninitialized rather than consuming a
    partial file.
  - Advisory locks do not stop a second host from using an imported identity; enforce same-state
    local exclusion mechanically and retain the explicit distributed duplicate-identity warning in
    public backup documentation and later operator/cutover procedures.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including cryptographic vectors, filesystem
     failure/recovery, redaction, configuration, and every repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so identity/configuration code, tests, dependency policy, and plan
     bookkeeping form one reviewable change.

## 2026-08-27 — Verified DTO semantic conversion

Implemented the sole reducer-ready transition from a complete `VerifiedSupportedRecord` into
`VerifiedSemanticFact`, retaining exact signed content and event evidence alongside the domain
fact. Exhaustive conversion covers all 48 canonical/control families, every nested semantic type,
typed scopes and authority roles, bounded domain values, namespace-safe causal references, and
intrinsic author/scope/body/routing agreement without moving historical authorization into the
protocol layer. Added shared exact fixtures, both published-vector transitions, deep nested-value
checks, adversarial subject/scope/domain-bound/reference-alias cases, compile-fail trust-state
proofs, and semantic fuzz seeds. Format, architecture/behavior/causal/protocol verifiers, workspace
check/build/test/doctests, strict Clippy, dependency policy, four protocol targets, fuzz smoke,
whitespace, and unchanged Go build/vet/fresh full regression gates pass.

### Original plan entry

- **[protocol/high] Convert verified v1 DTOs into semantic facts** — Implement typed scope and
  causal-reference conversion, all family-specific intrinsic agreement checks, and the lossless
  transition from every verified canonical/control v1 DTO to its `SemanticFact` family. Add
  exhaustive bidirectional semantic fixtures, authority/scope/reference adversarial matrices, and
  conversion fuzz/property coverage. Complete this split package when all 48 semantic mappings are
  executable and no invalid or unsupported record can reach reduction as a falsely verified fact.

  **Implementation plan**

  - Add failing public API tests first that drive the two published vectors and one valid DTO for
    every family through `VerifiedSupportedRecord` into a new reducer-ready owner. Assert exact
    `FactKind`, event-ID identity, author key/address, authored milliseconds, scope, causal parents,
    typed authority roles, representative nested fields, and retained event/content evidence.
  - Introduce `VerifiedSemanticFact` as the sole successful conversion result. It owns the validated
    domain `SemanticFact` together with its prior verified DTO/event evidence, exposes immutable
    audit bytes and a fact borrow, and has no constructor from raw, parsed, prefix-supported, failed,
    or verified-unsupported values.
  - Implement small total primitive converters from fixed DTO types into every opaque domain ID/key,
    nonempty bounded text, provider/session, operation correlation, locator scheme/value, mailbox
    and installation address, timestamp, positive sequence, context, message, resource, binding,
    activity/runtime status, initial state, and remote result. Map validation failures to closed
    redacted semantic-conversion classes without carrying attacker text.
  - Convert signed scope and references before payload construction. Require exact protocol/scope
    isolation and common author agreement; erase canonical/control reference namespaces only after
    DTO direction checks, reject decoded-ID collisions, construct bounded sorted parent sets, map
    every closed authority role, require exact authority-parent linkage, and retain no wire string
    or generic JSON representation in domain state.
  - Implement one exhaustive numeric/body match producing all 48 `SemanticPayload` variants with no
    fallback or string inference. Preserve all optional values, ordered relay/resource arrays,
    correlations, project bindings, message provenance, runtime uncertainty, and remote command
    results exactly while applying the domain's narrower bounds such as error-code length.
  - Enforce family-specific intrinsic agreement at conversion: installation/creator/device/project
    roots versus verified author/key, family message purpose and output identity/project, local/peer/
    account/control audience and sender/source relations, peer-self exclusion, project primary
    membership/home, request target-home scope, receipt/outcome home signer, and every catalog
    body/envelope equality that requires no historical parent lookup. Leave ancestry, referenced
    family/subject, active authority, and reducer-state sufficiency to reduction.
  - Reuse a single shared integration fixture catalog so every exact DTO body is converted and its
    resulting payload kind is bidirectionally checked against `FactKind::ALL`; add deep equality
    checks for published family 1/46 mappings and focused representatives of every nested type.
    Add adversarial scope/author/body/route/reference/domain-bound cases and compile-fail trust-state
    examples proving unsupported and prefix-only types cannot expose a semantic fact.
  - Extend the structure-aware DTO fuzz target through semantic conversion and seed intrinsic-edge
    inputs. Run the pinned fuzz smoke plus format, all spec/architecture verifiers, workspace check/
    build/test/doctests, strict Clippy, root/fuzz dependency policy, four-target protocol checks,
    whitespace, and unchanged Go build/vet/fresh regression suite before recording.

  **Risks and mitigations**

  - Erasing a control/canonical namespace too early can alias causal references; validate direction
    first and reject any decoded-ID collision before constructing the domain's namespace-free parent
    set or authority map.
  - Wire short text is sometimes wider than its semantic destination, especially `ErrorCode`; run
    every domain constructor and return a stable `domain-value-invalid` class instead of truncating,
    normalizing, or retaining an invalid DTO as a fact.
  - Intrinsic checks can accidentally duplicate historical authorization policy; restrict protocol
    conversion to equalities available in the current signed envelope/body and leave parent family,
    subject, ancestry, frontier, active-membership, and aggregate decisions to pure reducers.
  - Forty-eight large match arms can silently swap same-width IDs; centralize primitive converters,
    name every body field explicitly, and pair the exhaustive kind table with exact deep checks for
    nested/address/provenance-heavy families.
  - Dropping wire evidence after semantic conversion would weaken replay/audit guarantees; retain
    the verified DTO owner inside `VerifiedSemanticFact` and expose exact event/content bytes without
    serializing domain structs back into the protocol.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including all 48 mappings, intrinsic and
     domain-bound adversarial matrices, fuzz smoke, and every repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so conversion code, tests, fuzz assets, and plan bookkeeping form one
     reviewable change.

## 2026-08-27 — SQLite owner and immutable verified fact corpus

Implemented the sole Rust SQLite ownership boundary as a non-cloneable synchronous store actor
with a bounded typed mailbox, one private bundled-rusqlite connection, acknowledged lifecycle,
redacted failures, and architecture enforcement preventing SQLite from escaping `hq-store`.
Storage v1 uses a private WAL database, exact application/version/schema identity, read-only
foreign-state inspection, strict permissions and sidecar checks, immutable exact signed-event
rows, and normalized parent and typed-authority indexes. Append is transactional and exact-replay
idempotent while unequal valid signatures under one event ID fail closed. Corpus load is bounded,
snapshot-consistent, and reruns the complete raw-to-semantic trust pipeline before comparing every
stored index. Reopen, late-parent, collision, failpoint, rollback, actor backpressure/lifecycle,
schema/version, foreign-database nonmutation, corruption, permissions/symlinks, unsupported and
tampered evidence, compile-fail isolation, workspace, dependency, four-target, fuzz, and unchanged
fresh Go regression gates pass.

### Original plan entry

- **[storage/high] Establish the SQLite owner and immutable verified fact corpus** — Design the
  fresh Rust schema by durability class, implement one dedicated synchronous store thread owning one
  `rusqlite` connection behind a bounded typed mailbox, and durably append exact verified signed
  events plus normalized causal indexes without exposing SQL, rows, transactions, or connections.
  Apply secure database/directory modes and SQLite settings, reject incompatible/corrupt/Go schemas,
  make equal duplicate append idempotent and unequal identity reuse fail closed, and re-run the full
  protocol trust pipeline when loading the corpus. Test open/close/reopen, rollback-on-drop and
  failpoints, mailbox shutdown/backpressure, permission/schema/corruption errors, exact evidence
  retention, and compile-fail SQL isolation. Complete this package when later rebuild and mutation
  work can depend on one typed authoritative corpus owner with no alternate database-opening path.

  **Implementation plan**

  - Add failing public-contract tests first for secure fresh open, append, equal duplicate replay,
    deterministic load, explicit close, drop/reopen, exact signed-event retention, and reconstruction
    of reducer-ready semantic facts through the complete raw/parse/signature/dispatch/DTO/semantic
    trust pipeline. Use only valid signed protocol fixtures at the public boundary.
  - Specify the fresh storage v1 schema and durability classes in `docs/rust/storage.md`. Use a
    nonzero SQLite application ID and one exact user version; classify the signed event corpus as
    immutable canonical knowledge, parent and authority edges as normalized rebuildable indexes,
    and reserve materialized, operational, ephemeral, and rejected-input ownership without copying
    a Go table or schema number.
  - Add current reviewed `rusqlite` 0.40.2 with its bundled modern SQLite implementation only to
    `hq-store`. Keep the connection, statements, transactions, SQL strings, schema row codecs, and
    failpoint seam private. Add protocol accessors only for already-verified namespace/family
    metadata needed to cross-check stored rows; do not serialize domain values generically.
  - Implement a non-cloneable `Store` owner whose worker thread opens and exclusively owns one
    connection. Route coarse append/load/close commands over a bounded `sync_channel` with typed
    one-shot replies, acknowledge startup and shutdown, close intake before joining, and map worker
    loss or panic to stable redacted error classes. Make the public API safe to share by reference
    without exposing a database handle or table-shaped CRUD.
  - Create or validate only the immediate state directory as `0700`, reject directory/database
    symlinks and unsafe existing modes, create the database as `0600` before SQLite opens it, and
    reject non-regular files. Configure foreign keys, WAL, full durability, busy timeout,
    `trusted_schema=OFF`, and defensive mode on the owning connection; verify the effective safety
    settings and keep sidecars inside the private directory.
  - On open, distinguish a new empty file from existing state. Initialize the entire v1 schema in
    one exclusive transaction, set and verify application/user versions, run integrity and foreign
    key checks, and fail closed on foreign, Go-era, newer, older, malformed, or corrupt databases.
    Never migrate, reset, repair, attach, or otherwise interpret a non-v1 database in normal node
    startup.
  - Append one `VerifiedSemanticFact` transactionally as exact signed event bytes plus verified
    namespace/family, sorted parent edges, and unique typed authority edges. Return `Inserted` for a
    new identity and `AlreadyPresent` only when every immutable byte and indexed value agrees;
    classify any unequal reuse of one fact ID as a closed identity collision. Use foreign keys and
    checks for byte widths, namespace/family ranges, and authority roles without requiring parents
    to have arrived.
  - Load facts in fact-ID order, bound every database blob/count before allocation, and rerun raw
    bounds, strict outer parsing, event-ID/signature verification, supported dispatch, canonical v1
    DTO verification, and semantic conversion. Cross-check the stored ID, namespace, family,
    parents, and authorities against the reconstructed owner, rejecting tampering rather than
    trusting or silently rebuilding immutable/indexed rows.
  - Add internal transaction failpoints after each append write group and prove rollback-on-error
    and rollback-on-drop with a reopened connection. Add adversarial tests for unsupported or
    tampered evidence, unequal row reuse, partial indexes, foreign keys, corrupt bytes, foreign/Go
    schemas, wrong versions, unsafe modes, symlinks, missing parents, bounded-mailbox saturation,
    dropped replies, shutdown acknowledgement, and worker failure. Add compile-fail examples showing
    callers cannot obtain connections, execute SQL, or manufacture corpus facts from raw bytes.
  - Run format, all architecture/spec verifiers, workspace check/build/test/doctests, strict
    Clippy, dependency policy, four-target core/protocol checks, whitespace, and unchanged Go
    build/vet/fresh full regression suite before recording.

  **Risks and mitigations**

  - Opening through SQLite before validating the path can follow a symlink or create a permissive
    file; validate with non-following metadata, create privately first, revalidate the opened file,
    and rely on the already-held node state lock for same-user replacement exclusion.
  - WAL can weaken durability or leave privately scoped sidecars with surprising settings; pin and
    verify full synchronous WAL behavior, private directory/database modes, foreign keys, defensive
    mode, and explicit close/checkpoint behavior, then exercise reopen after abrupt actor drop.
  - Treating normalized indexes as authority could let corruption change reduction; reconstruct
    every semantic fact from exact signed bytes and compare all stored indexes before returning a
    corpus. The following repair package may replace rebuildable rows only after this check has a
    deliberate repair entry point.
  - A convenient raw append or query API could bypass trust states and later become a second commit
    path; accept only `VerifiedSemanticFact`, return only typed corpus owners/outcomes, keep every
    lower-level function private, and pin the isolation boundary with compile-fail documentation.
  - A synchronous bounded channel can deadlock shutdown if ownership and producers are ambiguous;
    keep one non-cloneable owner, permit shared references through ordinary `Arc`, acknowledge every
    accepted request, close intake before join, and state-test saturation, disconnect, panic, and
    last-owner drop.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including persistence, trust replay,
     corruption, failpoints, permissions, actor lifecycle, and every repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so store code, tests, specification, dependencies, and plan
     bookkeeping form one reviewable change.


## 2026-08-27 — Complete-batch snapshot and repair foundation

Added one actor-owned complete-batch oracle that reverifies the immutable corpus once under an
explicit authority policy and runs all four authoritative reducers. Added typed complete and
normalized reduction-index snapshots, storage v2 structural tables, exhaustive closed reason
codecs, bounded fail-closed row loading, and one explicit repair transaction that replaces only
rebuildable rows and verifies exact readback before commit. Empty/populated snapshots, direct
reducer equality, missing and late parents, unusable authority, root conflicts, idempotence,
reopen, immutable corpus preservation, stale-index semantics, corruption, dropped replies,
nine rollback checkpoints, and successful retry are covered. Formatting, architecture/behavior/
causal/protocol verifiers, workspace check/build/tests/doctests, strict Clippy, dependency policy,
all four release-target core/protocol checks, both fuzz smokes, whitespace, and unchanged Go
build/vet/fresh regression gates pass.

### Original plan entry

- **[storage/high] Implement the complete-batch snapshot and repair foundation** — Reverify the
  immutable corpus under explicit local authority policy, run all four authoritative batch reducers,
  and expose one typed in-memory `CompleteSnapshot`. Extend the fresh schema with normalized reverse
  dependencies, per-domain decisions and diagnostic edges, dependency order, and presentation order;
  transactionally replace only those rebuildable rows and expose a typed persisted-index snapshot
  independent of SQL. Prove idempotent repair, rollback at each replacement group, exact equality
  with fresh reports, and immutable-corpus preservation across missing parents, conflicts, late
  authority, corruption, and reopen. Complete this package when later projection codecs can consume
  one batch oracle and repair transaction without inventing another reduction or database path.

  **Implementation plan**

  - Add failing store-contract tests first for an explicit `AuthorityPolicy`, empty and populated
    complete snapshots, all four reducer reports, a typed normalized persisted-index view, repair,
    repeated-repair equality, close/reopen equality, and late-parent reconsideration. Construct every
    public input through valid signed protocol trust states and compare against direct
    `reduce_complete` calls over the same semantic facts.
  - Keep one immutable-corpus read and one SQLite transaction on the owning actor thread. Reverify
    exact event bytes using the existing corpus loader, clone only reducer-ready `SemanticFact`
    values at the pure boundary, and run `AuthorityReducer`, `ConversationReducer`, `AgentReducer`,
    and `ProjectReducer` with the same caller-supplied local authority policy. Return a
    `CompleteSnapshot` containing typed reports, never database rows or serialized domain structs.
  - Extend fresh storage schema identity to the next unreleased version and add only rebuildable
    structural tables: reverse dependency edges including missing vertices, per-domain fact
    decisions, missing/unusable dependency diagnostics, failed authority roles, conflict
    participants, deterministic dependency positions, and reducer-owned presentation positions.
    Do not add domain projection-value, operational, receipt, revision, outbox, or staging rows yet.
  - Define stable store-owned enums for the four reduction domains, six decision statuses, framework
    versus domain reason codes, and typed diagnostic records. Implement exhaustive explicit mappings
    from every authority, conversation/activity, agent, and project reason variant, including nested
    authority reasons and authority-role parameters; do not persist `Debug` text or generic Serde
    encodings.
  - Normalize each fresh report into one representation-independent `ReductionIndexSnapshot` with
    ordered domain/fact decisions, dependency order, presentation order, reverse dependencies,
    missing dependencies, unusable dependency statuses, failed roles, and conflict participants.
    Store and load that normalized type through private row codecs with fixed-width identity checks,
    closed integer vocabularies, uniqueness constraints, and count bounds before allocation.
  - Implement `repair(policy)` as one transaction that computes the complete oracle before writes,
    deletes only rebuildable structural rows, inserts every replacement group, reads the persisted
    normalized index back inside the transaction, compares it to the fresh normalization, and commits
    only on exact equality. Never update or delete canonical facts, parent/authority corpus rows,
    schema metadata, or future durable operational tables.
  - Make ordinary complete-snapshot reads pure with respect to storage and make persisted-index reads
    fail closed on absent, partial, unknown, duplicate, oversized, or cross-domain rows. Give explicit
    `NotRepaired` and `RebuildableStateCorrupt` classifications so callers can choose repair rather
    than silently accepting or mutating a damaged view.
  - Add failpoints after each delete/insert/verification group and prove rollback preserves the prior
    complete index. Add adversarial tests for missing and late parents, unusable authorities,
    identity/root conflicts, corrupted statuses/reasons/positions/diagnostic edges, cross-domain
    leakage, repeated repair, corpus byte/count equality before and after repair, reopen, dropped
    replies, and a repair failure followed by a successful retry.
  - Update `docs/rust/storage.md` with the batch-oracle, schema-evolution, repair, and public-boundary
    contract. Run format, all architecture/spec verifiers, workspace check/build/test/doctests,
    strict Clippy, dependency policy, four-target core/protocol checks, fuzz smoke, whitespace, and
    unchanged Go build/vet/fresh full regression suite before recording.

  **Risks and mitigations**

  - Four independent reports can drift if they see different corpus snapshots or policies; load and
    reverify once, capture one explicit policy value, and derive all reports before the repair
    transaction writes any rebuildable row.
  - Persisting reducer `Debug` output would make schema compatibility depend on prose and Rust type
    names; use closed store-owned integer codecs with exhaustive matches and typed public enums.
  - Repair can accidentally become a hidden mutation or erase future operational state; centralize
    the exact rebuildable table set, test canonical row/byte equality, and reject any delete target
    outside that private allowlist.
  - A partial or corrupt normalized index must not masquerade as an authoritative snapshot; validate
    row cardinality and referential/domain consistency, compare repaired rows to fresh normalization
    before commit, and require explicit repair after typed corruption detection.
  - Cloning the corpus into four current reducer reports has bounded but nontrivial memory cost; keep
    this correctness-first batch oracle, record the limit, and leave shared-report or incremental
    optimization to the later measured scaling package.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** the implementation in proportion to risk, including batch equality, repair rollback,
     corruption, actor lifecycle, and every repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so batch snapshot, repair, tests, specification, and plan bookkeeping
     form one reviewable change.

## 2026-08-27 — Complete authority projections

Added a typed SQL-independent authority snapshot and storage v3 relational projections for every
authority aggregate, frontier, support edge, history child, relay hint, conflict, and local account
selection. Extended the existing repair transaction so structural and authority indexes replace and
verify atomically, including authority-specific rollback failpoints, strict closed codecs, bounded
loading, valid-looking corruption detection, stale-read behavior, policy replacement, repeated
repair, and reopen equality. Added validated reducer reconstruction boundaries and documented
ownership, query, codec, and repair semantics. Formatting, architecture/behavior/causal/protocol
verifiers, workspace check/build/tests/doctests, strict Clippy, dependency policy, all four
release-target core/protocol checks, both 512-run fuzz smokes, whitespace, and unchanged Go
build/vet/fresh full regression gates pass.

### Original plan entry

- **[storage/high] Persist complete authority projections** — Extend the fresh schema and existing
  repair transaction with every authority aggregate frontier, typed projection value, and transitive
  support edge. Expose one typed `AuthorityProjectionSnapshot` independent of SQL layout and prove it
  equals the authority report in the same complete oracle before commit, after reopen, and after
  repeated repair. Cover installations, mailboxes, peer routes and blocks, mailbox capabilities,
  accounts, device memberships, local account selection, conflicts, late authority, and corruption.
  Complete this package when later services can query the full persisted authority view without
  rerunning a reducer or decoding database-shaped values.

  **Implementation plan**

  - Add failing public store contracts first for empty and populated authority snapshots, equality
    with `CompleteSnapshot::authority()` frontiers/projections/support, repair readback, repeated
    repair, close/reopen, policy replacement, stale-index behavior after append, and explicit repair
    reconsideration. Keep all integration inputs in valid signed protocol trust states; use private
    codec-unit fixtures only to exhaust variants that are not yet convenient signed scenarios.
  - Add a typed `AuthorityProjectionSnapshot` owning ordered maps for every
    `AuthorityAggregateKey` frontier, every `AuthorityProjectionKey`/`AuthorityProjection` pair, and
    every projection support set. Its public API returns domain types and aggregate queries, never
    SQL rows, discriminants, serialized blobs, or connection-shaped operations.
  - Advance the unreleased fresh schema identity and add rebuildable authority tables. Use explicit
    variant rows and normalized child rows for installations, mailboxes, route candidates/blocks/
    relays/frontiers, capabilities/revokes/observations, accounts, memberships/grants/relays/
    acceptances/revokes/frontiers, account-selection candidates, aggregate frontiers, and projection
    support. Keep canonical evidence, operational state, and later conversation/agent/project rows
    outside this package's delete and insert groups.
  - Implement exhaustive private integer/row codecs for aggregate keys, projection keys, mailbox/
    route/membership states, optional bounded text, installation/mailbox addresses, signing and
    encryption keys, error codes, relay resource schemes/values, and every authority projection
    variant. Reconstruct through validated domain constructors, reject unknown kinds, wrong widths,
    invalid UTF-8/bounds, duplicates, orphan child rows, cross-key rows, and impossible variant/key
    pairings, and check stored counts before allocating.
  - Derive the expected typed authority snapshot directly from the already-computed complete oracle.
    Extend the one repair transaction with authority clear/insert/readback groups and require exact
    authority equality together with the structural-index equality before commit. Include authority
    rows and a store-owned digest/count marker in stale persisted reads without making ordinary reads
    mutate or silently recompute them.
  - Add rollback failpoints around each new authority replacement group. Prove failures retain the
    preceding structural and authority snapshots and that an explicit retry succeeds. Add corruption
    tests for every table family and key/value vocabulary plus valid-looking support/frontier/child
    leakage; prove repair preserves exact immutable corpus bytes and never touches unrelated rows.
  - Exercise root identity conflicts, route set/block/frontier restoration, capability revoke and
    observed-action history, account roots, membership pending/active/revoked and regrant frontiers,
    local selection candidates/active choice, late authorities, policy change, reopen, and
    idempotence. Round-trip every authority projection/key/state codec exhaustively even when a
    higher-level signed scenario already covers it.
  - Update `docs/rust/storage.md` with authority ownership, query, codec, and repair semantics. Run
    format, all architecture/spec verifiers, workspace check/build/test/doctests, strict Clippy,
    dependency policy, four-target core/protocol checks, fuzz smoke, whitespace, and unchanged Go
    build/vet/fresh full regression suite before recording.

  **Risks and mitigations**

  - Nested route and membership histories can be flattened inconsistently; use one typed snapshot as
    the codec oracle, explicit parent keys and ordinals, foreign keys, and exact readback equality.
  - Reusing reducer types at the public boundary must not make their memory layout a file format;
    persist only explicit store-owned rows and reconstruct values through typed constructors.
  - Policy-dependent account selection can drift from policy-independent history; store the policy in
    the existing repair marker and replace all authority rows atomically whenever policy changes.
  - Expanding repair can accidentally weaken the structural rollback guarantee; make the authority
    groups part of the same transaction and prove every new failpoint leaves the prior complete pair.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** authority codec, equality, repair, rollback, corruption, lifecycle, and every
     repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so authority rows, typed queries, tests, specification, and plan
     bookkeeping form one reviewable change.

## 2026-08-27 — Complete conversation and activity projections

Added a typed SQL-independent conversation/activity snapshot and storage v4 relational projections
for all aggregate frontiers, threads, messages and state history, peer receipts, action groups,
activity snapshots and completed records, retention, and support. Composite key columns retain every
typed namespace component behind recomputed private digests; closed codecs validate optional shapes,
ordered children, causal relations, full-width nonzero sequences, bounds, counts, and cross-key
invariants. Extended one repair transaction across structural, authority, and conversation packages
with rollback checkpoints, exact readback, valid-looking corruption detection, staleness, policy
replacement, repeated repair, and reopen coverage. Formatting, architecture/behavior/causal/protocol
verifiers, workspace check/build/tests/doctests, strict Clippy, dependency policy, all four
release-target core/protocol checks, both 512-run fuzz smokes, whitespace, and unchanged Go
build/vet/fresh full regression gates pass.

### Original plan entry

- **[storage/high] Persist conversation and activity projections** — Add explicit fresh schema rows
  and private codecs for every conversation, message-state, action-group, activity, retention,
  frontier, support, and conflict projection. Extend the same complete repair transaction and expose
  typed conversation/activity query snapshots whose equality is independent of SQL layout. Cover
  missing conversation history, message state, answers/cancellations, delivery evidence, activity
  coalescing, equal sequence conflicts, retention compaction, corruption, reopen, and repeated
  repair. Complete this package when fresh conversation reports equal persisted typed queries exactly.

  **Implementation plan**

  - Add failing public store contracts for empty and populated typed conversation snapshots, exact
    equality with `CompleteSnapshot::conversation()` frontiers/projections/support, repair readback,
    repeated repair, stale rows after append, explicit reconsideration, close/reopen, and authority-
    policy replacement. Keep public fixtures as valid signed protocol evidence and use private
    relational fixtures to exhaust values not economical to express through one signed history.
  - Add a SQL-independent `ConversationProjectionSnapshot` owning ordered aggregate frontiers,
    projection values, and transitive support. Expose only reducer/domain identities and values;
    keep connections, row discriminants, surrogate identities, and storage-specific encodings
    private to `hq-store`.
  - Advance the unreleased fresh schema identity and add explicit rebuildable tables for message-
    identity, thread, message-state, and composite activity frontiers; thread roots, answers,
    cancellations, pairwise causal relations, and ready order; message content, state frontier, and
    peer receipt evidence; action-group entries/final answer; selected and completed activity;
    session retention order; and support rows for all six projection-key variants. Do not place
    durable operational state, canonical evidence, or later agent/project projections in these
    replacement groups.
  - Implement exhaustive private codecs for aggregate and projection key variants, fixed identities,
    mailbox addresses, bounded content/short/provider/session/error text, optional recipients,
    message purpose/presentation, optional correlation/project scope, activity kind/status,
    nonzero full-width sequence numbers, booleans, causal relations, ordered children, and retention
    counts. Reconstruct all validated domain values through constructors; reject unknown codes,
    wrong widths, bad UTF-8/bounds, invalid option shapes, duplicate positions, orphan/cross-key
    children, impossible key/value pairings, and oversized row counts before allocation.
  - Derive the expected typed snapshot directly from the conversation report in the existing
    complete oracle. Add conversation clear/insert/readback verification to the same repair
    transaction as structural and authority state, with store-owned counts and a digest covering
    every explicit conversation row so valid-looking mutation fails closed. Ordinary reads must
    validate all three persisted packages and never silently rerun reduction.
  - Add conversation repair failpoints and prove each failure preserves the preceding structural,
    authority, and conversation snapshots and allows explicit retry. Corrupt every conversation
    table family with constraint-valid mutations and prove typed reads reject the package until
    repair while immutable corpus bytes and unrelated authority rows remain exact.
  - Exercise questions/asynchronous roots, answers and cancellations with every causal relation,
    reversible archive/restore and absorbing rejection, peer receipt evidence, typed action groups,
    snapshot coalescing, durable completed records, equal-sequence/runtime conflicts, composite
    activity keys, and the 200-item progress-retention boundary. Round-trip every closed scalar and
    projection variant even when integration scenarios overlap.
  - Update `docs/rust/storage.md` for conversation ownership, query, codec, staleness, and atomic
    repair semantics. Run format, all architecture/spec verifiers, workspace check/build/test/
    doctests, strict Clippy, dependency policy, four-target core/protocol checks, both fuzz smokes,
    whitespace, and unchanged Go build/vet/fresh full regression suite before recording.

  **Risks and mitigations**

  - Composite activity keys and optional message fields can alias if flattened loosely; use explicit
    typed columns, closed option shapes, full parent keys on children, and exact snapshot readback.
  - SQLite signed integers cannot represent every positive `NonZeroU64`; persist sequences as exact
    fixed-width big-endian bytes and validate nonzero reconstruction.
  - Ordered ready/action/retention lists can silently duplicate or gap; store unique zero-based
    positions and require contiguous order plus set/cardinality invariants on load.
  - A conversation-only repair failure must not weaken earlier atomicity; perform all three package
    replacements in one transaction and exercise failpoints before and after conversation verify.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** every conversation/activity codec, repair, rollback, corruption, lifecycle, and
     repository-wide gate above.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so typed queries, relational rows, tests, specification, and plan
     bookkeeping form one reviewable change.

## 2026-08-27 — Complete named-agent projections

Added a typed SQL-independent agent snapshot and storage v5 relational projections for every
aggregate frontier, permanent name and lifecycle view, immutable session binding, repository
context history, session selection, rename register, direct-session binding, and transitive support
set. Explicit composite columns and recomputed private digests prevent namespace aliasing; closed
codecs validate nested optional resource locators, bounded text, lifecycle and register invariants,
counts, ownership, and exact key/value pairings. Extended the single repair transaction across the
structural, authority, conversation, and agent packages with exact readback, rollback checkpoints,
staleness, policy replacement, reopen, and constraint-valid corruption coverage. Formatting,
architecture/behavior/causal/protocol verifiers, dependency policy, locked workspace check/build/
tests/doctests, strict Clippy, all four required core/protocol targets, both 512-run fuzz smokes,
whitespace, and the unchanged Go vet/build/fresh full regression suite pass.

### Original plan entry

- **[storage/high] Persist complete agent projections** — Add explicit fresh schema rows and private
  codecs for every agent name, lifecycle, session binding, repository context, selection, rename,
  and direct-session view, including every aggregate frontier and projection support set. Extend the
  typed query boundary and the existing atomic repair transaction, prove exact equality with the
  fresh agent report before and after reopen and repeated repair, and cover name/mailbox/session
  conflicts, retirement, runnable selection, late authority, policy replacement, and corruption.
  Complete this package when later services can query the entire persisted agent view without SQL
  knowledge or reducer execution.

  **Implementation plan**

  - Add failing public contracts for empty and populated typed agent snapshots, exact report
    equality, repair readback, repeated repair, staleness after append, explicit reconsideration,
    policy replacement, and close/reopen. Use valid signed fixtures for public flows and private
    relational fixtures to exhaust nested variants and register states.
  - Add `AgentProjectionSnapshot` with ordered typed frontiers, all seven projection-key/value
    variants, and transitive support. Keep schema identities, row kinds, digests, and connections
    behind the store boundary.
  - Advance the unreleased fresh schema identity and add explicit rebuildable master/child tables
    for composite aggregate and projection keys; name claims/subjects; lifecycle claims, names,
    mailboxes, retirements and selected session; session bindings; repository context history and
    frontier; selection candidates and frontier; rename candidates and frontier; direct-session
    binding facts; aggregate frontiers; and projection support.
  - Implement exhaustive closed codecs for short/provider/session text, fixed IDs, mailbox and
    session identities, lifecycle, booleans, optional agent/mailbox/session/name values, repository
    contexts and optional resource locators/branches, ordered or set children, and all key/value
    pairings. Reconstruct validated domain values through constructors, bound counts before
    allocation, and reject unknown, malformed, duplicate, orphan, cross-key, or impossible rows.
  - Derive expected rows only from the existing complete oracle. Add agent clear/insert/readback,
    counts, and whole-package digest verification to the same transaction as structural, authority,
    and conversation state. Validate every earlier package during ordinary agent reads and preserve
    explicit stale-until-repair behavior.
  - Add agent insert/verification failpoints and prove rollback preserves the preceding four-package
    snapshot and retry succeeds. Apply constraint-valid corruption to every agent table family and
    prove it fails closed until repair without changing corpus bytes or earlier projection rows.
  - Round-trip name conflict/retirement state, active/conflicted/retired lifecycle, runnable and
    conflicted selections, session binding conflicts, context histories/frontiers, resolved clear
    and conflicted rename states, and named/unnamed/conflicted direct sessions. Update storage docs
    and run every repository-wide gate before recording.

  **Risks and mitigations**

  - Provider/session and mailbox composites can alias when flattened; store every component in
    explicit columns behind a recomputed private key digest.
  - Repository contexts contain multiple optional resource locators; use closed presence shapes and
    validated scheme/value constructors for every nested field.
  - Derived booleans and optional active values can contradict child history; validate lifecycle,
    conflict, frontier, selection, rename, and binding invariants during reconstruction.
  - Extending repair must preserve prior atomic guarantees; add agent checkpoints inside the same
    transaction and verify all four packages after every failed replacement.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** agent codecs, equality, rollback, corruption, lifecycle, and all repository gates.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so agent rows, queries, tests, specification, and bookkeeping form one
     reviewable change.

## 2026-08-27 — Complete project projections and repair equality

Added a typed SQL-independent project snapshot and storage v6 relational projections for every
project, resource claim, assignment, accepted input, dispatch, output, remote command, aggregate
frontier, and transitive support set. Explicit composite columns and private digests preserve home,
resource, agent, and command namespaces; closed codecs validate full-width sequences, nested message
and assignment provenance, optional shapes, lifecycle/claim/runnable rules, stage/status vocabularies,
row bounds, counts, and exact key/value pairing. Closed the single repair transaction across the
structural index and all four reducer reports with exact readback, rollback checkpoints, signed
project history, conversation coexistence, staleness, policy replacement, reopen, and corruption
coverage. Formatting, architecture/behavior/causal/protocol verifiers, dependency policy, locked
workspace check/build/tests/doctests, strict Clippy, all four required core/protocol targets, both
512-run fuzz smokes, whitespace, and the unchanged Go vet/build/fresh regression suite pass.

### Original plan entry

- **[storage/high] Persist project projections and close complete repair equality** — Add explicit
  fresh schema rows and private codecs for every project/resource/assignment/input/dispatch/output/
  remote-command view, including every aggregate frontier and support set. Extend typed queries and
  the one repair transaction across all four reducer domains, prove every persisted report equals a
  fresh complete reduction before and after reopen and repeated repair, and cover conflicts, late
  authority, runnable state, compacted activity coexistence, and corrupted rebuildable rows.
  Complete this package when the immutable corpus rebuilds every public projection exactly without
  touching durable operational state.

  **Implementation plan**

  - Add failing public contracts for empty and populated typed project snapshots, exact report
    equality, repair readback, repeated repair, staleness after append, explicit reconsideration,
    policy replacement, close/reopen, and coexistence with already-persisted compacted activity.
    Use signed protocol histories for public flows and exhaustive private relational fixtures for
    nested states that would otherwise require a large causal scenario.
  - Add `ProjectProjectionSnapshot` with ordered typed frontiers, all five projection-key/value
    variants, and transitive support. Expose project/domain types only; keep schema identities,
    surrogate digests, connections, and row discriminants private to `hq-store`.
  - Advance the unreleased fresh schema identity and add explicit rebuildable tables for composite
    project, home-qualified resource, agent-assignment, input, dispatch, output, and command
    aggregate keys; project roots/heads/forks/resources/claims/conflicts/assignments; accepted
    inputs; dispatch attribution; output provenance and content; remote command stages; aggregate
    frontiers; and projection support.
  - Implement closed codecs for every fixed identity, bounded short/content/provider/session/error
    value, resource locator and health, lifecycle, assignment phase and binding, optional
    predecessor/brief/primary/assignment/recipient/correlation/project/runtime shapes, message
    purpose/presentation, output status, command result/stage, booleans, sets, maps, and full-width
    input/dispatch sequences. Reconstruct through validated domain constructors, bound counts before
    allocation, and reject unknown, malformed, duplicate, orphan, cross-key, or impossible rows.
  - Derive expected project rows only from the complete oracle. Add project clear/insert/readback,
    counts, and a whole-package digest to the same transaction as structural, authority,
    conversation, and agent state. Validate every earlier package during ordinary project reads and
    preserve explicit stale-until-repair behavior.
  - Add project insert/verification failpoints and prove rollback preserves the preceding complete
    five-package snapshot and retry succeeds. Apply constraint-valid corruption to every project
    table family and prove reads fail closed until repair without changing corpus bytes, durable
    operational state, or any earlier projection package.
  - Round-trip open/closing/closed and archived projects; resource health, primary choice, active
    claims and cross-project conflicts; configuring/runnable/blocked assignments and cardinality
    conflicts; accepted inputs; conflicted dispatches; current/late/conflicted output with complete
    typed messages; and queued/received/terminal/conflicted remote commands. Update the storage
    specification and run every repository-wide gate before recording.

  **Risks and mitigations**

  - Resource and assignment namespaces can alias when flattened; retain every typed component in
    explicit columns and recompute private digests from closed encodings.
  - Project views contain deeply nested optional values and derived flags; use closed presence
    shapes and verify lifecycle, claimability, assignment-runnable, stage, status, and membership
    invariants during reconstruction.
  - SQLite signed integers cannot cover all `u64` sequences; persist them as fixed-width big-endian
    bytes and validate exact round-trip semantics.
  - Closing the repair set must not weaken prior rollback or staleness guarantees; replace and
    verify all five packages in one transaction with checkpoints around project insertion and
    verification.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** project codecs, equality, rollback, corruption, lifecycle, and all repository gates.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so project rows, queries, tests, specification, and bookkeeping form
     one reviewable change.

## 2026-08-27 — Durable mutation receipts, revisions, and outbox intents

Advanced the unreleased Rust database to storage v7 and added strictly relational durable
operational state outside the repair allowlist. Typed mutation receipts bind a stable command ID,
exact request digest, closed committed/rejected result kind, bounded exact result bytes, and
revision. Change revisions preserve the complete `u64` domain as big-endian bytes, increment
monotonically, and fail explicitly on exhaustion. Per-recipient outbox intents retain exact signed
canonical bytes and creating revision behind a fact/installation identity. Equal writes deduplicate
and unequal receipt or outbox reuse fails closed. Bounded actor queries return only typed receipts,
revisions, and intents; no connection or row shape escapes.

Unit and public contracts cover inclusive byte bounds, closed result codecs, full-width revision
round-trip and exhaustion, exact receipt/outbox equality and collisions, invalid query limits,
actor reply loss, repair preservation, close/reopen, and strict schema identity. Architecture,
behavior, causal, protocol, and dependency verifiers; locked workspace format/check/build/tests/
doctests/Clippy; all four required targets; both 512-run protocol fuzz smokes; whitespace checks;
and the unchanged Go vet/build/fresh test suite pass.

### Original plan entry

- **[storage/high] Add durable mutation receipts, revisions, and outbox intents** — Define bounded,
  typed storage contracts and a versioned SQLite representation for mutation request digests and
  exact result receipts, monotonic change revisions, and per-recipient canonical outbox intents.
  Preserve exact canonical bytes for retry, make receipt and outbox identities collide on unequal
  content, keep these operational rows outside repair, and expose read models without leaking SQL
  shapes. Add round-trip, full-`u64`, reopen, collision, and repair-preservation tests. Complete this
  work when the operational primitives are durable, strictly decoded, and independently usable by
  the later common transaction engine.

  **Implementation plan**

  - Add bounded typed exact-result and exact-canonical-byte owners plus public mutation receipt and
    outbox intent read models. Keep construction invariants and raw storage identities encapsulated.
  - Advance the unreleased schema identity and add strict receipt, singleton revision, and
    per-fact/per-installation outbox tables. Encode revisions as fixed-width big-endian bytes.
  - Implement private closed row codecs, equality-aware inserts, explicit collision classes,
    monotonic allocation with exhaustion, and deterministic bounded reads through the store actor.
  - Prove repair never touches operational state and that all three primitives retain exact values
    through close/reopen. Update the normative storage contract and schema adversarial assertions.
  - Run every Rust architecture/spec/dependency/build/test/lint/target/fuzz gate and the unchanged Go
    vet/build/test suite before recording completion.

  **Risks and mitigations**

  - SQLite integers cannot represent every `u64`; use eight-byte big-endian revisions and exercise
    `u64::MAX` plus exhaustion explicitly.
  - Opaque result or event bytes could permit unbounded work; validate inclusive byte budgets at
    construction and row decoding, and reject outbox query limits outside `1..=1024`.
  - Repair could accidentally become destructive as its allowlist grows; keep operational tables
    out of every clear path and prove exact values before and after repair and reopen.
  - Stable identities can hide unequal retries; compare every retained field on duplicate writes
    and return typed mutation or immutable-identity conflicts on any mismatch.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** bounds, codecs, collisions, repair preservation, reopen, and all repository gates.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so schema, operational types, actor queries, tests, specification,
     and bookkeeping form one reviewable change.

## 2026-08-27 — Common atomic canonical ingest

Replaced the production append-only path with one actor-owned atomic ingest operation. A single
immediate SQLite transaction now deduplicates exact verified evidence, persists causal indexes,
reverifies the transaction-visible corpus, runs the complete four-domain correctness oracle,
replaces and verifies every rebuildable projection package, allocates a full-width revision,
derives admitted-scope per-recipient outbox intents, records stable canonical commit lineage, and
commits. Exact replay returns the original revision before any derived or operational write.

Capacity-one revision invalidations publish only after a new commit and coalesce without blocking
durability. File-backed failpoint coverage reopens every pre-commit boundary to the exact old state,
then proves retry reaches the complete new state; post-commit response loss replays the original
answer without churn. Public contracts cover immediate materialization, batch/repair equality,
fanout filtering and exact bytes, duplicate/reopen behavior, and observer coalescing. Storage v8 and
the normative persistence specification describe the new common engine and lineage.

Architecture, behavior, causal, protocol, and dependency verifiers; locked workspace format/check/
build/tests/doctests/Clippy; all four required targets; both 512-run protocol fuzz smokes; whitespace
checks; and the unchanged Go vet/build/fresh test suite pass.

### Original plan entry

- **[storage/high] Implement the common atomic canonical-ingest transaction** — Replace the
  append-only remote path with one transaction that deduplicates a verified semantic fact, appends
  canonical evidence and dependency rows, performs complete reduction as the initial correctness
  oracle, replaces every projection package, derives durable per-recipient outbox intent, allocates
  a change revision, commits, and then emits a non-blocking revision invalidation. Add write- and
  commit-boundary failpoints, duplicate-ingest and local/remote common-path equality fixtures, and
  reopen assertions proving every interruption leaves the old valid state or the new valid state,
  never a hybrid. Keep repair from altering receipts, revisions, or outbox state.

  **Implementation plan**

  - Add failing public contracts for one-call ingest, duplicate replay with the original revision,
    complete projection visibility without repair, exact repair equality, admitted-scope fanout,
    bounded coalesced invalidation, and reopen. Remove the production append-only actor operation.
  - Advance the unreleased schema with a durable fact-to-commit-revision record so duplicate ingest
    is an exact no-op with a stable answer. Keep this operational lineage outside repair.
  - Refactor corpus reverification and complete repair replacement to operate inside a caller-owned
    SQLite transaction. Reuse those private primitives from both explicit repair and common ingest
    without nesting transactions or exposing row-shaped operations.
  - In one immediate transaction, append/deduplicate exact evidence and causal indexes, run the full
    four-domain reducer oracle, replace and verify all rebuildable packages, allocate a revision,
    derive admitted peer/account/control recipients from the post-reduction authority view, persist
    exact outbox intents and canonical commit lineage, and commit.
  - Publish a capacity-one coalesced wake only after commit. The wake carries no projection data and
    observer backpressure or disconnection must never delay or fail a durable transaction.
  - Add failpoints after every canonical, index, projection, revision, outbox, lineage, and commit
    boundary. Reopen after each interruption and prove equality with either the complete old state
    or complete new state; simulate response loss after commit and prove replay is unchanged.
  - Update projection/repair contracts and the normative storage specification, then run every Rust
    and unchanged Go repository gate before recording completion.

  **Risks and mitigations**

  - Calling repair helpers from ingest can accidentally nest SQLite transactions; split replacement
    into a transaction-owned core and keep transaction creation only at coarse public boundaries.
  - Forwarding rejected or unresolved facts leaks or amplifies unusable input; create outbox intents
    only when the fact has an admitted reducer decision and derive recipients from authorized views.
  - Duplicate delivery can create revision churn and invalidation storms; retain the original fact
    revision and return before every rebuildable or operational write on exact replay.
  - A post-commit response or observer failure is not a rollback opportunity; persist lineage before
    commit, make notifications non-blocking, and prove lost-response replay returns the same result.
  - Full-batch reduction is deliberately expensive at this stage; document it as the correctness
    oracle and leave affected-closure optimization to the immediately following incremental task.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** atomicity, replay, fanout, invalidation, repair equality, reopen, and all gates.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so engine, schema, tests, specification, and bookkeeping form one
     reviewable change.

## 2026-08-27 — Transaction-consistent local fact-backed mutations

Added a bounded local mutation contract that looks up stable receipts before invoking a one-shot
decision callback against the complete snapshot held by the same immediate SQLite transaction. The
protocol now owns typed semantic authoring for all 48 fact families, deterministic canonical
encoding, BIP-340 signing with explicit auxiliary randomness, and ordinary trust-transition
reverification. Committed plans enter the exact remote canonical-ingest core; rejected decisions
atomically allocate only a revision and exact result receipt. Exact retries return retained bytes
without deciding, signing, revision churn, outbox work, or observer wakes, while changed digests
fail as typed conflicts.

File-backed failpoints cover receipt lookup, snapshot, decision, signing, every common ingest write,
rejected revision, receipt persistence, and both sides of commit. Reopen assertions prove every
pre-commit interruption retains the complete old state, response loss replays the complete new
state, repair preserves receipts, and local and remote ingest produce identical canonical evidence,
indexes, projections, outbox, and revisions. Signer mismatches and reducer-unadmitted local facts
fail closed with no durable state. Protocol, trust-transition, and storage specifications describe
the ownership and transaction boundaries.

Architecture, behavior, causal, protocol, and dependency verifiers; locked workspace format/check/
build/tests/doctests/Clippy; all four required targets; both 512-run protocol fuzz smokes; whitespace
checks; and the unchanged Go vet/build/fresh test suite pass.

### Original plan entry

- **[storage/high] Implement transaction-consistent local fact-backed mutations** — Add a bounded
  typed store request that first looks up a stable mutation receipt and rejects a reused command ID
  with a different request digest, otherwise decides against the snapshot held by the same SQLite
  transaction using only explicit time, ID, and randomness inputs, signs the deterministic event
  plan, and enters the common canonical-ingest path. Persist the exact typed result receipt in the
  same commit. Keep unsigned local operational mutations on explicitly separate paths with the same
  retry discipline where client-visible. Add signing, rejection, lost-response replay,
  same-ID/different-input conflict, local/remote equality, and failpoint/reopen tests. Complete this
  work when retries return the original result and every crash recovers to an old or new valid state
  without a hybrid.

  **Implementation plan**

  - Add a protocol-owned `CanonicalEventPlan` built only from typed domain author, time, scope,
    causal references, and payload values. Exhaustively translate all 48 semantic families into the
    private v1 DTO model, encode canonically, sign with caller-supplied BIP-340 auxiliary randomness,
    and rerun the ordinary dispatch/DTO/semantic trust transitions before returning verified input.
  - Add a bounded `LocalMutationRequest` carrying stable command ID and digest, explicit authority
    policy, a non-secret shared signer handle, and a one-shot pure decision callback. The callback
    receives the complete snapshot read inside the transaction and returns either a typed event plan
    plus exact committed result bytes or exact rejected result bytes; it has no store or ambient
    clock/randomness access.
  - Refactor common canonical ingest into a caller-owned transaction core. Keep remote ingest as a
    thin immediate-transaction wrapper, and call the identical append/reduce/project/outbox/lineage
    core from local mutation without nesting or duplicating commit behavior.
  - In one immediate transaction, load and strictly compare any retained receipt before invoking the
    decision callback; otherwise reverify and reduce the transaction-visible corpus, decide, sign,
    pass committed plans through the common core, persist the exact committed or rejected receipt at
    the transaction revision, and commit. Exact retry returns the original receipt without deciding,
    signing, allocating a revision, emitting outbox work, or waking observers.
  - Allocate a revision for a durable rejection as well as a committed fact, publish the existing
    capacity-one invalidation only after a new local transaction commits, and fail closed if a local
    committed plan unexpectedly names canonical evidence without its atomic receipt. Keep repair and
    future unsigned operational/saga mutations on separately named APIs rather than an optional-fact
    mutation path.
  - Add public and file-backed failpoint contracts for signing, policy rejection, receipt replay,
    same-ID/different-digest conflict, dropped actor response, local/remote common-engine equality,
    every local/common write boundary, reopen, repair preservation, and secret-redacted diagnostics.
    Update the protocol/storage specifications and run every Rust and unchanged Go gate.

  **Risks and mitigations**

  - Letting application code serialize private DTOs would invert protocol ownership; expose one
    typed semantic event plan and keep all family-specific wire translation inside `hq-protocol`.
  - Deciding before opening the transaction would authorize against a stale snapshot; invoke the
    one-shot callback only after receipt lookup and complete transaction-visible reduction.
  - Passing secret bytes through actor messages would widen the secret boundary; pass only a shared
    signer capability whose API exposes public key and signing, then verify its output normally.
  - A retry could rerun nondeterministic planning or create revision/outbox churn; compare the stable
    digest first and return retained exact bytes before the callback or common engine.
  - Rejected commands have no canonical fact but still create durable reconciliation state; allocate
    their receipt revision atomically and wake readers only after commit.
  - Splitting local and remote implementations can drift; make both wrappers call one transaction-
    owned common engine and compare complete corpus, indexes, projections, outbox, and revision in a
    deterministic two-database fixture.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** authoring, signing, decisions, retry/conflict, atomicity, equality, reopen, and gates.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so protocol authoring, transaction engine, tests, specifications, and
     bookkeeping form one reviewable change.

## 2026-08-27 — Incremental materialization and indexed conversation queries

Added a deterministic affected-dependency graph spanning causal edges, aggregate membership,
projection support, and conflict participants. Incremental ingest grows the closure over old and
fresh graphs, checks every changed decision and projection against it, stages the batch oracle in an
isolated schema, applies a typed primary-key row diff in foreign-key-safe order, and requires exact
live equality before operational writes. Explicit repair remains a complete replacement and now
recreates and strictly validates the same dependency and conversation-order indexes.

Added closed thread and provider-session conversation keys, reducer-derived induced-graph ordering,
and a bounded typed page API. Versioned cursors bind the conversation-key digest to a stable fact,
page scans read at most `limit + 1` indexed order rows, and targeted hydration reads only the selected
message or durable-activity projections. Contracts cover late parents, unrelated-row protection,
malformed and cross-key cursors, equal-time mixed pagination, repair/reopen equality, and a 1,000-entry
fixture whose later pages use covering indexes without loading or sorting complete history.

Architecture, behavior, causal, protocol, and dependency verifiers; locked workspace format/check/
build/tests/doctests/Clippy; all four required targets; both 512-run protocol fuzz smokes; whitespace
checks; and the unchanged Go vet/build/fresh test suite pass. The timing-sensitive Go node test also
passed 10 isolated repetitions after one transient full-suite failure.

### Original plan entry

- **[storage/high] Add incremental reduction, repair equality, and scalable conversation queries** —
  Implement deterministic dependency indexes and affected-closure selection, then patch projections
  incrementally while continuously comparing with fresh batch rebuilds. Build a conversation-local
  order index or stable cursor derived from the reducer comparator so page concatenation equals
  canonical order and later pages do not load or sort complete history. Add generated late-parent,
  high-fanout authority, duplicate-ingest, equal-time mixed-entry, reopen/repair, and large multi-page
  work tests plus performance budgets. Complete this work when incremental, batch, and repair views
  are identical and query work meets the documented scaling gate.

  **Implementation plan**

  - Extend complete reducer reports with deterministic aggregate membership and derive one
    conservative affected-dependency graph from causal edges, shared aggregate membership,
    projection support, and conflict participants. Calculate growth from the union of the persisted
    and fresh graphs so disappearing conflicts and supports remain selectable; expose a pure
    fixed-point affected-closure operation and persist the exact graph as rebuildable state.
  - Keep `reduce_complete` as the executable policy oracle instead of creating a second partial
    reducer with duplicated domain rules. For each new fact, build the fresh report, verify that
    every changed decision and projection is covered by the selected closure, stage its normalized
    relational representation in an isolated in-memory database, and apply only added, removed, or
    changed rows to the live transaction in foreign-key-safe order. Read the complete live result
    back and require exact batch equality before revision, outbox, lineage, or receipt writes.
  - Advance the unreleased schema for affected dependencies and conversation-local order. Derive
    closed thread or provider-session conversation keys relative to the explicit local authority
    policy, select only reducer-projected message and activity winners, and run the one canonical
    Kahn comparator independently for each key. Repair recreates the same rows from the same batch
    report, while incremental row diffing changes only affected conversations.
  - Add a bounded typed conversation-entry query with a closed message/activity union, a 200-item
    maximum, and a strict versioned cursor binding the conversation-key digest to the last fact ID.
    Resolve the cursor through the indexed conversation-local position, select at most `limit + 1`
    rows, and hydrate only those exact typed projection rows; never load a complete snapshot or sort
    history on a page request.
  - Add differential contracts for every incremental prefix, late parents and descendants,
    high-fanout authority changes, aggregate/global conflicts, exact duplicates, repair, response
    loss, and reopen. Protect unrelated rows with aborting SQLite triggers, compare all normalized
    reports and conversation-order rows, and exercise equal-time mixed entries across many pages.
  - Specify explicit work budgets: page limits are `1..=200`, every page reads no more than
    `limit + 1` order rows and hydrates no more than `limit` projections, cursor lookup and page
    selection use covering indexes, and a later page over at least 1,000 entries performs no
    canonical-event scan, complete projection load, or in-memory ordering. Run every Rust and
    unchanged Go repository gate.

  **Risks and mitigations**

  - A narrowly modeled affected graph could miss a retraction caused by authority or global
    constraints; include old and new causal/aggregate/support/conflict edges, fail closed when any
    normalized change lies outside the closure, and compare the persisted result with batch on
    every commit.
  - Per-table patch code would duplicate nearly one hundred strict relational codecs; materialize
    expected rows through the existing insertion codecs and use one schema-introspected typed row
    differ that rejects unsupported SQLite value classes and updates only exact differences.
  - Dynamic row patching can violate foreign keys transiently; delete removed rows child-first,
    insert additions parent-first, defer checks within the owning transaction, and verify all
    package digests plus typed snapshots before commit.
  - Filtering one global presentation order is not generally equal to ordering an induced
    conversation graph; derive each conversation order directly with the reducer comparator.
  - Offset or position-only cursors can silently cross conversations or drift after a late insert;
    bind cursors to a key digest and stable fact identity, resolve the current local position, and
    reject malformed, stale, or cross-conversation anchors.
  - Hydrating through the existing full-snapshot loader would defeat pagination; add targeted strict
    projection loaders and prove bounded row work separately from full repair validation.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** closure selection, row-local patching, equality, cursor safety, query work, reopen,
     repair, and all repository gates.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so reducer indexes, storage patching, queries, tests, specifications,
     and bookkeeping form one reviewable change.

## 2026-08-27 — Transport-independent application services and ports

Moved normalized authority, conversation, agent, project, and unified conversation-page values to
the application consumer boundary and added one revisioned authoritative snapshot. Added closed
application errors, strict versioned mutation-result encoding, pure transaction-snapshot fact
decisions, typed completed/rejected/uncertain attempts, stable external-effect envelopes, and narrow
query, commit, wake, relay, harness, resource, and revision-observation capabilities. The stateless
service keeps post-commit scheduling separate from durable success and enforces pending registration,
snapshot read, explicit acknowledgement activation, and failure cancellation ordering.

Storage now depends inward on application and provides a signer/policy-configured gateway. One store
actor request reads revision plus all four projection packages, fact plans enter the existing atomic
local mutation engine, and retained application receipt bytes are strictly decoded with result-kind
agreement. Contracts cover exact replay, changed digests, pure commit/rejection, corrupted retained
results, committed wake failure, accepted/rejected/uncertain external effects, subscription traces,
and gateway equality. Architecture and four-target CI now include the pure application crate.

Architecture, behavior, causal, protocol, and dependency verifiers; locked workspace format/check/
build/tests/doctests/Clippy; all four required targets including application; both 512-run protocol
fuzz smokes; whitespace checks; and the unchanged Go vet/build/fresh test suite pass.

### Original plan entry

- **[application/high] Implement transport-independent application services and ports** — Implement
  identity/account, mailbox/conversation, peer/relay configuration, synchronization, agent/session,
  project, query, mutation, and subscription use cases over consumer-owned ports rather than SQL- or
  transport-shaped interfaces. Keep command decisions pure and represent external side effects as
  explicit requests/results with stable identities and rejected/committed/uncertain outcomes. Add
  focused fakes and use-case tests for authorization, mutation replay, error classification, and
  authoritative snapshot semantics. Complete this work when no supported use case requires local
  RPC, SQLite, Nostr, terminal, filesystem, or provider-specific knowledge.

  **Implementation plan**

  - Make `hq-application` own representation-independent authority, conversation, agent, and project
    projection snapshots, the unified conversation entry, and one revisioned authoritative snapshot.
    Move these client-facing values out of `hq-store`; keep reducer values as the shared semantic
    vocabulary and keep complete reduction/index diagnostics private to persistence and repair.
  - Define closed application error classes and stable codes, retryable fact-mutation requests with
    explicit command ID, digest, authored inputs, and signing randomness, a pure one-shot decision
    against an application snapshot, and typed committed/rejected/uncertain attempts. Own a strict
    versioned receipt-result codec in the application crate so durable replay never serializes Rust
    structs, transport DTOs, diagnostics, or prose.
  - Declare narrow consumer-owned capabilities for `QueryDomain`, `CommitFacts`, `PublishWake`,
    `ConfigureRelays`, `ControlHarness`, `InspectResource`, and `ObserveRevisions`. Give external
    relay, synchronization, runtime, and resource operations stable operation identity and digest,
    and represent accepted, rejected, and reconcilable uncertain outcomes without provider,
    filesystem, network, SQL, or runtime vocabulary.
  - Implement one stateless application service over a capability bundle. Expose authoritative
    refresh, indexed conversation pages, fact-backed identity/account/mailbox/conversation/agent/
    project mutations, relay configuration, explicit synchronization, neutral session control,
    resource inspection, and two-phase subscriptions. A committed mutation schedules relay work but
    remains committed if that post-commit wake is coalesced or unavailable.
  - Close the subscription revision race at the service boundary: register a pending observer before
    loading its acknowledged authoritative snapshot, cancel it if the query fails, return it still
    pending for the transport to acknowledge, and activate only through a separate call after the
    acknowledgement has been written.
  - Add an `hq-store` application gateway configured with explicit local authority and signer
    capabilities. Atomically load revision plus all four persisted projection packages on the store
    actor, translate application fact plans only at the protocol-owning adapter edge, execute the
    existing transaction-consistent mutation path, strictly decode retained application receipts,
    and classify store failures without leaking storage types.
  - Add scripted capability fakes and contracts for pure authorization decisions, exact replay and
    changed-digest conflict, post-commit wake independence, committed/rejected/uncertain external
    effects, error classification, register/query/ack ordering, query-failure cancellation, store
    gateway equality, and compile-time dependency direction. Update the application/workspace/
    acceptance specifications and run every Rust and unchanged Go repository gate.

  **Risks and mitigations**

  - Returning store-owned snapshots would make every client depend on persistence; move only the
    semantic projection owners to application and let storage re-export them for compatibility while
    keeping SQL codecs private.
  - A generic external-effect trait would hide materially different retry and reconciliation rules;
    keep relay configuration, wake, harness control, and resource inspection as separate capability
    traits over shared stable operation envelopes.
  - Reporting a wake or observer failure as a mutation failure could cause duplicate local commands;
    make the durable receipt authoritative and report post-commit scheduling separately.
  - Reading revision and projections through separate actor calls admits a torn authoritative view;
    add one actor-owned aggregate query and test that its revision and packages come from the same
    serialized store point.
  - Persisting arbitrary result bytes would defer corruption to retry time; use one bounded explicit
    application codec and require the stored result kind and decoded outcome to agree.
  - Activating a subscription before the client sees its snapshot can lose an invalidation between
    acknowledgement and observation; model pending registration and activation as distinct calls and
    prove their ordering with a trace fake.

  **Post-Plan Execution Steps**

  1. **Implement** the expanded plan above completely.
  2. **Test** ports, decisions, replay, uncertainty, authoritative snapshots, subscriptions, the
     store gateway, and all repository gates.
  3. **Commit** all task changes with a Conventional Commit message.
  4. **Update this plan** by removing this completed entry from **Next Up** and appending its exact
     text, implementation plan, risks, and completion evidence to `COMPLETED.md`.
  5. **Amend the same commit** so application contracts, storage adaptation, tests, specifications,
     and bookkeeping form one reviewable change.

## 2026-08-27 — Bounded local API v1 wire protocol

Specified and implemented a storage-independent local API v1 codec with an independently versioned
handshake, build metadata, four-byte big-endian framing, strict canonical JSON, closed typed message
families, inclusive allocation bounds, lifecycle and domain operations, authoritative snapshots and
conversation pages, stable mutation identity, exact retry payloads, and revision-only invalidations.
Plain wire records use idiomatic public fields while invariant-bearing values retain validated
constructors.

Added unsigned canonical-plan content transitions for all 48 semantic fact families without
granting signer or evidence authority. Mutation requests bind exact plan bytes and auxiliary signing
randomness with a domain-separated SHA-256 digest before conversion to application fact plans.
Normative and executable specifications cover message families, bounds, canonicality, truncation,
unknown data, incompatible versions, digest mismatch, and the subscription and replay rules.

The full locked workspace format/check/tests/Clippy suite, architecture and specification
verifiers, dependency checks, four supported compilation targets, both 512-run protocol fuzz
smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[local-api/high] Specify local API v1 and implement its bounded typed wire contract** — Write
  `docs/protocol/local-api-v1.md` and implement the independently versioned handshake, build
  metadata, length-delimited framing, strict decoding and allocation bounds, lifecycle operations,
  domain queries, explicit request/result/error DTOs, stable mutation identity and exact retry
  payload, authoritative snapshot/page representations, and revision-only invalidation messages.
  Keep DTOs and conversions in `hq-local-api`; domain and application types must not become wire
  formats. Test every message family, boundary size, truncation, trailing/unknown data, incompatible
  versions, noncanonical encodings, and mutation digest mismatch. Complete this work when a client
  and a server can interoperate semantically through one storage-independent v1 codec.

  **Implementation plan**

  - Specify a four-byte big-endian length prefix around canonical compact JSON, reject declared
    oversize before body work, deny unknown DTO fields/variants, and require byte-identical
    re-encoding so equivalent but noncanonical JSON cannot create replay aliases.
  - Own closed v1 handshake, request, response, error, snapshot/page, effect, subscription, and
    invalidation DTOs in `hq-local-api`, with semantic validation after deserialization and named
    inclusive bounds for every variable-size value.
  - Reuse `hq-protocol`'s exhaustive canonical semantic DTO mapping for unsigned mutation plans,
    while exposing a distinct encode/decode transition that cannot produce verified evidence.
    Bind exact plan bytes and auxiliary signing randomness into a domain-separated SHA-256 request
    digest and convert only validated bytes into an application `FactPlan`.
  - Cover every top-level and application-operation family with semantic round trips, exercise
    incremental framing, all framing/canonicality failures, inclusive bounds, digest tampering, and
    all 48 unsigned semantic plan families. Add executable checks that the normative specification
    names every bound and the replay/subscription/trust rules.
  - Update workspace and acceptance documentation, run focused protocol gates, then run the full
    locked workspace, architecture, dependency, fuzz, target, whitespace, and unchanged-Go gates.

  **Risks and decisions**

  - Standard Rust enum encodings are not a protocol contract; every union has explicit lowercase
    tags, including a dedicated success/error response union instead of serializing `Result`.
  - Plain owned wire records expose documented public fields for idiomatic matching/destructuring;
    only true invariant wrappers keep private fields. Both encoding and decoding revalidate the
    complete message, so public DTO construction cannot bypass the wire boundary.
  - Unsigned canonical-plan content deliberately shares semantic spelling with signed canonical v1
    but carries no signer or fact identity. Only the node's later sign-and-verify transition may
    produce admissible evidence.
  - Snapshot DTOs are a bounded, closed client query representation. They are neither reducer Rust
    layouts nor storage rows; server-session conversion and race ordering remain in the immediately
    following package.
  - The incremental decoder retains at most one maximum frame. Socket adapters must feed bounded
    reads and close after any framing violation rather than accumulating attacker-controlled input.

  **Post-Plan Execution Steps**

  1. Implement the expanded plan above completely, preserving the independent protocol and trust
     boundaries.
  2. Test every message family, semantic-plan family, bound, malformed input class, and replay
     invariant, then run repository-wide gates.
  3. Commit the implementation and specifications with one Conventional Commit.
  4. Remove this exact entry from **Next Up**, append it verbatim with completion evidence to
     `COMPLETED.md`, and amend the same commit before advancing to server sessions.


## 2026-08-27 — Race-safe local server sessions and bounded invalidation fanout

Implemented a transport-independent negotiated server session over application capabilities plus a
separate node-lifecycle capability. Every typed local API request family routes through exhaustive
application-to-wire conversions, responses are correlated, and only one response write may remain
unconfirmed. Session-owned single-use write tickets activate pending subscriptions only after the
acknowledgement frame is confirmed; response loss, disconnect, and drop cancel every owned pending
or active registration.

Added a fixed-capacity shared revision hub implementing the two-phase observer port. Publications
perform no I/O, filter unrelated topics, and retain at most one in-place coalesced notice per
subscriber with the maximum revision, unioned topics, and sticky full-snapshot requirement.
Application now owns a closed client projection catalog, while storage supplies indexed conversation
summaries from the same serialized authoritative snapshot and local API alone maps the catalog to
wire DTOs.

Contracts cover handshake and protocol order, all routed request families, revision races before and
after acknowledgement writes, single-use tickets, response loss, stale disconnects, capacity,
10,000-write slow readers, and concurrent publish/poll/cancel. Full locked workspace format/check/
build/tests/doctests/Clippy, architecture and specification verifiers, dependency policy, four
supported targets, both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh
tests pass.

### Original plan entry

- **[local-api/high] Implement race-safe server sessions and bounded invalidation fanout** — Build
  the transport-independent server-session library over application capabilities. Register each
  subscription before reading the revision its acknowledgement names, activate only after that
  acknowledgement is confirmed written, cancel pending/active registrations on disconnect, and
  coalesce each slow subscriber to one nonblocking pending wake carrying the newest revision,
  broad topics, and a full-snapshot flag. Route lifecycle and typed domain operations without any
  storage dependency. Test concurrent clients, revision races at every registration phase, stale
  sockets, slow/nonreading subscribers, cancellation, response loss, and commits while fanout is
  saturated.

  **Implementation plan**

  - Add an exhaustive local API v1 conversion module that is the sole boundary between wire DTOs
    and application/domain/reducer values. Convert authoritative projection packages into bounded
    snapshot items, bounded conversation queries/pages, exact mutation and effect requests/results,
    subscription topics, and closed redacted errors without exposing storage or serializing Rust
    layouts.
  - Define a narrow local lifecycle capability and a synchronous transport-independent server
    session. Require a completed handshake before requests, correlate every response, reject
    protocol-order violations, and represent response writes as explicit tickets whose successful
    confirmation performs subscription activation. A failed/lost write or disconnect cancels every
    pending and active registration owned by that session.
  - Implement a shared revision hub behind `ObserveRevisions`. Register bounded unique
    subscriptions as pending, record matching commits during every pending/active phase, expose
    invalidations only after activation, and retain at most one coalesced wake per subscription with
    the maximum revision, unioned broad topics, and sticky full-snapshot requirement.
  - Keep publication nonblocking and bounded under slow readers: reject excess subscribers at
    registration, never queue multiple wakes for one subscriber, filter unrelated topics unless a
    full snapshot is required, and make cancellation idempotent. Session polling consumes only its
    own active invalidations and stale session cleanup releases all capacity.
  - Add deterministic trace and concurrency contracts for handshake/order errors, every routed
    request family, acknowledgement-write activation, commits before snapshot/during write/after
    activation, lost responses, disconnect cancellation, saturated registration, slow and
    nonreading clients, coalescing, stale sockets, and concurrent publication/cancellation.

  **Risks and decisions**

  - Returning an acknowledgement is not proof that bytes reached the transport. Activation is a
    distinct post-write confirmation on an opaque session-owned ticket; tickets cannot be replayed
    across sessions or confirmed twice.
  - A channel per commit would make memory proportional to publisher speed. The hub stores one
    bounded aggregate per subscription and polling removes it, so publication performs no I/O and
    does not wait for readers.
  - Commits racing before the acknowledged snapshot read may produce a redundant invalidation but
    cannot create a gap. Commits after pending registration remain coalesced until write-confirmed
    activation.
  - Lifecycle control is a local composition capability, not an application/domain or storage
    concern. Keep it beside the server session so the later node package can supply ownership and
    drain semantics.
  - Snapshot conversion is intentionally exhaustive over closed reducer projection variants. Any
    future semantic variant must fail compilation until its stable local representation is chosen.

  **Post-Plan Execution Steps**

  1. Implement the expanded plan above completely with red race, routing, conversion, and capacity
     contracts first.
  2. Run focused local API tests, full locked Rust gates, architecture/dependency/specification
     checks, supported targets, fuzz smokes, whitespace checks, and the unchanged Go gates.
  3. Commit the implementation and documentation with one Conventional Commit.
  4. Remove this exact entry from **Next Up**, append it verbatim with completion evidence to
     `COMPLETED.md`, and amend the same commit before advancing to the reconnecting client.

## 2026-08-27 — Reconnecting local client replay and resubscription

Implemented one transport-independent reconnecting client state machine and narrow connect/write/
close adapter contract for every local frontend. It negotiates v1 on every generation, ignores
stale socket events, stops on explicit incompatibility, and emits deterministic exponential
backoff capped by validated policy. Ordinary requests retain correlation but are reported lost
rather than silently replayed; snapshot or registration failures reconnect from a clean base.

In-flight mutations retain their original request ID, command ID, digest, and complete encoded
frame, which is replayed byte-for-byte after ambiguous response loss. Changed command reuse is
rejected before transport, concurrent mutations are capped, and completed identities use a bounded
oldest-first window. Logical subscriptions survive connections, derive fresh registrations from
the client seed and server session, accept the acknowledgement snapshot as their base, and
coalesce revision wakes into repeated full refreshes until current.

Contracts cover loss before or after mutation commit through the shared durable receipt path,
exact replay, changed input, repeated loss and capped backoff, incompatible and stale sessions,
lost acknowledgements, early/coalesced invalidations, behind snapshots, resubscription, lifecycle
restart response loss, bounded history, and independent clients. Full locked workspace format/
check/build/tests/doctests/Clippy, architecture/dependency/specification verifiers, four supported
targets, both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[local-api/high] Implement reconnecting local client replay and resubscription** — Build the
  shared client state machine and transport adapter contract used by CLI, TUI, and local harness
  launchers. Negotiate on every connection, retry a lost mutation response with the exact stable ID
  and bytes, never retry changed input under an existing ID, detect stale sessions, reconnect with
  bounded backoff, re-register subscriptions, and request a fresh authoritative full snapshot
  before treating invalidations as current. Test disconnects before/after mutation commit,
  incompatible restarted servers, repeated connection loss, invalidation gaps, resubscription races,
  clean lifecycle restart, and two clients racing. Complete this work when all clients can use one
  protocol library without storage access and reconnect without duplicate mutations or revision
  gaps.

  **Implementation plan**

  - Define one transport-agnostic client state machine driven by explicit connection-generation,
    frame, write-failure, disconnect, and reconnect-timer events. Emit bounded connect/write/close
    actions rather than opening sockets or sleeping, and define the narrow adapter trait shared by
    CLI, TUI, harness launchers, and deterministic tests.
  - Negotiate local API v1 on every new connection and bind all later input to that connection's
    local generation plus the server's ephemeral session ID. Ignore stale connection events, stop
    automatic retry on explicit version incompatibility, and use deterministic exponential backoff
    capped at a configured maximum without consulting ambient time.
  - Retain each in-flight mutation as its exact canonical frame, request ID, command ID, and digest.
    Replay that byte-identical frame after negotiation when a response was lost, keep completed
    command identity/digest history bounded, and reject any changed request under an existing
    command ID before it reaches a transport.
  - Represent one logical broad-topic subscription intent independently from a connection. Derive a
    distinct registration ID from its stable client seed and negotiated server session, register it
    after every reconnect, and treat its acknowledgement snapshot as the fresh authoritative base.
    Ignore invalidations until that base is accepted; coalesce later revision-only notices while one
    authoritative refresh request is in flight and refresh again if the returned revision remains
    behind the newest notice.
  - Add deterministic scripted-adapter contracts for disconnect before/after mutation commit,
    byte-exact replay, changed-command rejection, repeated connection loss and capped backoff,
    incompatible restarted servers, stale frames, lost subscription acknowledgement,
    resubscription races, invalidation gaps, clean lifecycle restart, bounded retained identity
    history, and two independent clients racing one server.

  **Risks and decisions**

  - Reconstructing an equivalent mutation request could change framing or correlation bytes. Store
    and replay the original encoded frame verbatim; decode only the eventual correlated response.
  - Server session IDs are diagnostic, but combining the stable client subscription seed with the
    negotiated session ID creates a fresh registration identity without granting either value
    authority. This avoids stale-registration collisions across reconnects.
  - A revision invalidation is not a patch. Once received, the client marks its view stale and asks
    for a complete authoritative snapshot; it never infers missing rows or applies projection data
    from notifications.
  - Unbounded completed-command memory would turn stable identity defense into a leak. Retain a
    documented fixed-size oldest-first identity window while never evicting an in-flight mutation.
  - Backoff scheduling and connection I/O remain adapter responsibilities. The pure state machine
    supplies explicit capped delays and generation-scoped actions so tests need no clock or socket.

  **Post-Plan Execution Steps**

  1. Implement the expanded plan with red deterministic state-machine and scripted-transport tests
     first.
  2. Run focused client tests and every repository-wide Rust, target, fuzz, dependency, whitespace,
     and unchanged-Go gate.
  3. Commit the implementation and documentation with one Conventional Commit.
  4. Remove this exact entry from **Next Up**, append it verbatim with completion evidence to
     `COMPLETED.md`, and amend the same commit before advancing to node composition.

## 2026-08-27 — Secure node runtime paths and lifecycle ownership foundation

Implemented the node's first RAII composition owner over the process-lifetime state lock,
non-cloneable identity, unsigned local configuration, validated private runtime namespace, and
bounded store actor. Startup follows dependency order and ordinary ownership drops unwind every
partial failure. Checked shutdown closes and joins the store before runtime, identity, and state
ownership release; best-effort drop provides idempotent containment.

Added installation-qualified XDG runtime derivation with a state-local fallback, exact `0700`
validation, reserved socket/readiness paths, symbolic-link rejection, and a 103-byte portable Unix
socket pathname ceiling. Runtime preparation deliberately preserves unowned stale artifacts. The
pure lifecycle covers starting, store-revision readiness, draining, failure, stopped
acknowledgement, read/query/mutation/launch admission, and retained stop versus clean-restart
intent. Structured startup errors carry closed component/cause/action values and selected paths
without adapter or secret prose.

Contracts cover unsafe and linked runtime paths, portable length, stale-artifact preservation,
out-of-order lifecycle events, mutation rejection during drain, concurrent owners, missing
identity, store-open and runtime failure rollback, checked close, redacted diagnostics, and
immediate lock/store reacquisition. Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/specification verifiers, four supported core targets, both 512-run fuzz
smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Define secure runtime paths and lifecycle ownership foundations** — Add the
  first-release Unix runtime namespace, typed node phases and redacted startup diagnostics, and one
  RAII composition owner that acquires the installation lock, loads identity/configuration, opens
  the bounded store actor, and releases partial or complete ownership exactly once. Make readiness,
  mutation admission, drain, failure, and stop transitions explicit and deterministic. Test unsafe
  paths, symlinks, overlong socket paths, concurrent ownership, startup failure cleanup, mutation
  during drain, and exact store/lock release. Complete this work when later listener and component
  owners can depend on one executable lifecycle contract rather than infer process state.

  **Implementation plan**

  - Specify a separate private runtime root derived from an explicit absolute root, otherwise
    `$XDG_RUNTIME_DIR/hq`, with a private state-root fallback for macOS/service environments. Own
    stable socket and readiness-metadata paths, reject symbolic links and modes other than `0700`,
    and enforce the portable Unix socket pathname byte limit before binding.
  - Define closed `Starting`, `Ready`, `Draining`, `Failed`, and `Stopped` phases plus typed startup
    component, cause, and operator-action values. Keep filesystem and adapter prose out of stable
    errors; diagnostics may name only the explicitly selected state/runtime paths and safe public
    build/identity metadata.
  - Implement a pure lifecycle machine with explicit readiness revision, intake admission, orderly
    drain, terminal failure, and stop acknowledgement transitions. Reject new mutations and
    launches as soon as draining begins, make repeated stop/drain calls idempotent, and fail closed
    on out-of-order readiness or restart events.
  - Compose an RAII node foundation in dependency order: state-directory lock, identity,
    configuration, prepared runtime directory, and bounded store thread. Roll back through ordinary
    ownership drops on every startup failure and close the store before releasing runtime and state
    ownership on explicit shutdown.
  - Add deterministic pure and filesystem contracts for every transition and failure category,
    two concurrent owners, unsafe or stale artifacts, store-open failure, retry after failure,
    mutation admission during shutdown, redacted debug/display surfaces, and immediate reacquisition
    after clean close.

  **Risks and decisions**

  - A socket path accepted on Linux may exceed macOS `sockaddr_un`; enforce a documented portable
    byte limit at path construction instead of discovering it during bind.
  - Runtime cleanup before installation ownership could delete another process's socket. Path
    preparation validates only directories; stale socket/readiness cleanup is reserved for the
    later listener after the state lock is held.
  - A destructor cannot report store-close failure. Provide explicit checked shutdown for normal
    operation and retain idempotent best-effort drop solely as panic/early-return containment.
  - Readiness is not equivalent to lock acquisition or socket existence. Only the lifecycle owner
    may publish `Ready` after identity, configuration, store, and required component startup have
    all acknowledged success.

  **Post-Plan Execution Steps**

  1. Add failing lifecycle, path-security, concurrent-owner, and cleanup contracts first.
  2. Implement the pure machine and RAII foundation, then update node/workspace/acceptance docs.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before component
     composition.

## 2026-08-27 — Bounded node component ownership and ordered graceful drain

Implemented the sole node runtime owner over the RAII foundation, four closed component slots,
hierarchical cancellation, a fixed-capacity nonblocking mailbox, tracked native tasks, and the
shared revision hub. Startup acknowledges components in dependency order and rolls back every
partial failure in exact reverse order. Shutdown closes lifecycle and component intake, cancels the
root, drains each owner in normative order, escalates only failed or explicitly incomplete drains,
joins every accepted task, accumulates typed diagnostic issues, and always advances through store
and state-lock release.

Added transient complete application ports that delegate query/mutation to the store gateway,
revision observation to the hub, and relay, harness, and resource capabilities to their concrete
owners. The installation identity internally shares its signer through a reference-counted handle
without exposing secret bytes. Plain diagnostic/report records use idiomatic public fields and
startup failures are pattern-matchable; methods remain on invariant and ownership types.

Deterministic contracts cover all four startup failure positions and exact rollback traces,
retained hierarchical cancellation, sibling isolation, bounded mailbox saturation/closure,
receiver-drop closure, task saturation/closure/failure/panic joining, delegated capabilities,
mutation admission racing drain, restart intent, normative shutdown order, provider escalation,
cleanup errors that do not skip tasks or later owners, zero retained handles, and immediate lock
reacquisition. Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/behavior/specification verifiers, four supported core targets, both
512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Compose bounded component ownership and ordered graceful drain** — Build the sole
  node owner over the foundation, store gateway, revision hub, local-session registry, relay-manager
  port, harness-supervisor port, project-workflow port, hierarchical cancellation, bounded
  mailboxes, and tracked tasks. Start and acknowledge required components in dependency order;
  drain intake, clients, relay ingress, providers, workflows, store producers, tasks, store, and
  ownership in the normative order. Test startup rollback at every component, saturated mailboxes,
  mutation/drain races, task failure, provider escalation, restart, and leak-free exact-once close
  with deterministic fake adapters.

  **Implementation plan**

  - Define one closed component catalog (`LocalSessions`, `RelayManager`, `HarnessSupervisor`, and
    `ProjectWorkflows`) and one lifecycle trait with explicit start acknowledgement, stop-intake,
    drain result, and forced-stop escalation. Keep relay/harness/resource application capabilities
    on their existing narrow traits rather than hiding them in a generic component interface.
  - Implement a cloneable hierarchical cancellation token whose children observe parent
    cancellation without being able to cancel siblings, plus a fixed-capacity task tracker that
    rejects excess tasks, closes spawn intake before drain, joins every accepted thread, and reports
    panic or typed task failure without leaking handles.
  - Add a fixed-capacity nonblocking mailbox primitive with explicit `Full` and `Closed` outcomes.
    Use it as the component/test seam so every future producer must choose backpressure,
    coalescing, or rejection instead of creating an unbounded queue.
  - Compose `NodeOwner` over `NodeFoundation`, a shared revision hub, local-session registry, relay
    manager, harness supervisor, and project workflow manager. Start in dependency order with
    component-scoped cancellation children; on any failed acknowledgement, stop and drain every
    already-started component in reverse before store/lock release.
  - Close lifecycle admission first during shutdown, then stop local/client and external-work
    intake, cancel the root, drain local sessions, relay ingress/durable handoff, provider output,
    and project workflows in normative order, force-stop only components that request escalation,
    close/join tracked tasks, and finally close the store/foundation. Accumulate typed issues while
    continuing all later cleanup steps.
  - Adapt the non-cloneable installation identity to share one signer handle internally with the
    store gateway without exposing key bytes. Expose transient node application ports by delegating
    query/mutation to `StoreGateway`, revision observation to `RevisionHub`, and relay/harness/
    resource operations to their owning components.
  - Add scripted trace components and concurrency tests for every startup failure position,
    reverse rollback, mailbox saturation/closure, parent-child/sibling cancellation, tracker
    saturation/panic/failure, mutation admission racing drain, normative shutdown order, provider
    escalation, restart intent, component errors that do not skip cleanup, and immediate ownership
    reacquisition with no live accepted task.

  **Risks and decisions**

  - Returning early on the first drain error leaks later owners. Shutdown records stable issues and
    always advances through store close and lock release; the final report distinguishes clean,
    escalated, and failed stages.
  - A single cancellation flag would let one component stop siblings. Child tokens observe their
    parent but cancel only their own subtree; only the node owner holds the root cancellation right.
  - Treating a thread spawn as success before tracking its handle admits leaks. Capacity is reserved
    before spawn, every accepted handle is retained, and tracker intake closes before joining.
  - Future Tokio tasks and provider processes need different concrete escalation adapters, but the
    ownership protocol is the same. This package pins acknowledgements and order with deterministic
    threads/fakes; socket/runtime-specific execution remains in the following package.

  **Post-Plan Execution Steps**

  1. Add failing cancellation, mailbox, task, startup-rollback, and shutdown-order contracts first.
  2. Implement component ownership and delegated application ports; update lifecycle specifications.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before Unix listener
     and autostart work.

## 2026-08-27 — Private Unix listener and atomic readiness ownership

Implemented foundation-owned Unix listener binding inside the live installation lock, conservative
nonblocking stale-socket probing, owner-only socket modes, and identity-guarded removal. Accepted
connections are validated against Linux `SO_PEERCRED` or macOS `getpeereid` before protocol bytes
can enter the node. The platform credential seam is private, so callers cannot substitute the
security decision.

Added a bounded, canonical, versioned readiness record with idiomatic public data fields and
ready-only validation. Publication uses a unique `0600` same-directory temporary file, file sync,
atomic rename, directory sync, retained device/inode ownership, and duplicate boot-nonce rejection.
Startup rollback, checked shutdown, and drop close the listener and preserve every substituted or
unrelated path while continuing through store and state-lock release.

Deterministic filesystem and real Unix-socket contracts cover regular files, symlinks, unsafe modes,
absent/stale/live sockets, kernel same-user acceptance, scripted mismatch and missing-credential
failure, readiness bounds and canonical round trips, atomic replacement, lifecycle gating,
identity-changing cleanup, no temporary leaks, and immediate reacquisition. Full locked workspace
format/check/build/tests/doctests/Clippy, architecture/dependency/behavior/specification verifiers,
four supported core and node targets, both 512-run fuzz smokes, whitespace checks, and unchanged Go
vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Own the private Unix listener and atomic readiness artifacts** — Bind the
  installation-qualified Unix socket only through the live node foundation, reject unsafe artifact
  types and live competing listeners, replace only a proven stale socket, enforce owner-only modes,
  validate accepted peers as the effective user, and publish versioned readiness metadata through
  an atomic owned file. Cleanup must remove only the exact socket/readiness identities created by
  this owner. Test symlinks, ordinary files, stale/live sockets, replacement races, peer mismatch,
  atomic visibility, startup failure cleanup, path substitution, and immediate rebind on Linux and
  macOS.

  **Implementation plan**

  - Define closed redacted listener/readiness failure classes and a versioned readiness record with
    lifecycle phase, process identity, safe build identity, installation identity, authoritative
    revision, and a fresh non-authoritative boot nonce. Validate every decoded field and bound the
    complete file before allocating or parsing.
  - Make the foundation the only public bind entry point so socket cleanup authority cannot exist
    without the installation state lock. Bind directly when absent; when the address is occupied,
    reject symbolic links and non-sockets, probe the exact socket, classify a successful connection
    as live ownership, and unlink/retry only a connection-refused socket whose device/inode identity
    is unchanged.
  - Retain the bound socket identity and enforce `0600` after bind. Validate accepted Unix-stream
    credentials against the process effective user through a narrow platform adapter covering the
    first-release Linux and macOS targets; credential failure or mismatch closes the stream before
    protocol parsing.
  - Publish readiness only after `NodeOwner` has acknowledged foundation and components ready.
    Write a unique owner-only temporary file in the runtime directory, sync it, rename atomically,
    sync the directory, and retain the installed file identity. Never infer readiness from socket
    existence alone.
  - On explicit shutdown, startup rollback, or drop, close the listener before conditionally
    removing the socket and readiness file. Re-stat each path without following links and unlink
    only when its device/inode identity still matches this owner; preserve every substituted or
    unrelated artifact.
  - Add deterministic filesystem and real Unix-socket contracts for absent/stale/live paths,
    symlink and regular-file attacks, identity-changing cleanup races, private modes, same-user and
    scripted mismatched peers, readiness round-trip/atomicity/bounds, partial publication failure,
    exact cleanup, and immediate rebind/reacquisition.

  **Risks and decisions**

  - Blind stale-socket deletion can disconnect another process or delete a substituted path. Probe
    only while holding the state lock and compare the original device/inode immediately before the
    conditional unlink.
  - Filesystem modes alone do not prove the peer after descriptor transfer or platform quirks.
    Check kernel-reported credentials on every accepted connection before handing bytes to the
    local protocol.
  - A readiness file written before rename can be observed partially. Use same-directory atomic
    replacement, strict versioned decoding, and directory sync; the file remains diagnostic and
    never grants ownership or authority.
  - Destructors cannot surface cleanup failure. Provide checked cleanup for ordinary shutdown and
    identity-guarded best-effort drop for rollback/panic containment.

  **Post-Plan Execution Steps**

  1. Add failing artifact, stale/live probe, peer-credential, readiness, and cleanup contracts.
  2. Implement foundation-owned binding/publication and update lifecycle/security specifications.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before session-loop
     work.

## 2026-08-27 — Call-scoped server-session capabilities

Refactored `ServerSession` into connection protocol state plus revision-registration ownership. It
no longer stores generic application or lifecycle owners; each decoded request borrows a complete
application service and lifecycle capability only for synchronous dispatch. Negotiation, exact
single-use write tickets, close-after-version-rejection, post-write subscription activation,
coalesced invalidations, and disconnect cleanup remain in the session state machine.

Added a red-green contract proving one session can outlive and repeatedly use fresh temporary
capability bundles, plus a two-session contract proving drop cancels only the dropped session's
pending registration while the sibling remains active. Updated runtime specifications to pin the
central node-loop decision and avoid reference-counting concrete component owners solely for task
lifetimes. Plain protocol/result records remain public data; methods remain only on state-transition
and ownership types.

Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/behavior/specification verifiers, four supported core and node targets,
both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[local-api/high] Decouple server-session state from borrowed node capabilities** — Refactor the
  transport-independent `ServerSession` so it retains only connection protocol state, shared
  revision registrations, and write-transition ownership. Execute each decoded request against an
  application service and lifecycle capability borrowed only for that call. Preserve exact write
  confirmations, post-write subscription activation, close-after-rejection, coalesced
  invalidations, and disconnect cancellation while allowing the future node event loop to own
  non-`'static` component capabilities without reference-counting every owner.

  **Implementation plan**

  - Remove application/lifecycle type parameters and stored capability fields from `ServerSession`;
    keep negotiation, session identity, write tickets, subscription identities, and the cloned
    bounded revision hub as the session-owned invariant state.
  - Change request receipt to borrow one complete `Application` and `LifecycleControl` for the
    duration of synchronous dispatch only. Do not introduce trait objects, public adapter structs,
    or accessor-only wrappers; ordinary protocol/result records keep their public fields and the
    session retains methods only for ordered state transitions.
  - Preserve the rule that no second request is admitted while a response ticket is pending, a
    subscription becomes active only after its exact acknowledgement frame is confirmed written,
    and version rejection closes only after its exact response is confirmed.
  - Prove that one session can be driven repeatedly through freshly borrowed application bundles,
    that dropping those bundles leaves no retained borrow, and that disconnect/drop cancels all
    pending and active registrations without affecting sibling sessions.
  - Update protocol/runtime specifications to pin the central node-loop ownership decision before
    adding asynchronous socket tasks.

  **Risks and decisions**

  - Lifting the store and every component into `Arc` solely for task lifetimes would blur the sole
    owner and shutdown order. The central node loop will own components and lend capabilities only
    while processing a decoded request.
  - Moving all protocol state into the node would duplicate negotiation, ticket, and subscription
    invariants. `ServerSession` remains the one transport-independent state machine; only external
    capabilities become call-scoped.
  - Subscription registration and acknowledgement span a write. The revision hub remains retained
    session state so pending registration is cancelled on every write failure, disconnect, or drop.

  **Post-Plan Execution Steps**

  1. Add failing compile/runtime contracts for fresh borrowed bundles and retained session state.
  2. Refactor `ServerSession` and all callers without weakening existing protocol contracts.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before socket I/O.

## 2026-08-27 — Bounded authenticated Unix session I/O

Added the minimal Tokio Unix networking, I/O, runtime, macro, and synchronization features. A raw
accepted descriptor is now wrapped in an opaque `AcceptedLocalStream` only after foundation-owned
kernel same-user validation, and the session driver consumes that capability exactly once.

Implemented one joined per-connection future with a fixed encoded-frame queue, caller-bounded event
channel, bounded incremental decoding, ordered full-frame writes, and a close signal independent of
write capacity. Events are plain closed enums. Response tickets are emitted only after every byte
of the exact frame succeeds; malformed, oversized, noncanonical, truncated, failed, peer-closed, or
cancelled sessions terminate without retrying or falsely confirming a partial write. Either byte
loop ending drops its sibling and descriptor before one terminal event.

Red-green contracts cover partial and multiple input frames, oversized and truncated input, fixed
and closed write queues, invalid untracked messages, exact complete-frame ticket delivery, explicit
close while the queue is full, cancellation after a partial duplex write without completion, and
one joined terminal event. Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/behavior/specification verifiers, four supported core and node targets,
both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Drive bounded Unix session I/O** — Add Tokio-owned per-connection read/write tasks
  around the transport-independent session state, with bounded connection, decoded-message, and
  encoded-write capacity; incremental framing; exact `ServerSession` write confirmations only after
  full-frame completion; coalesced invalidations; and cancellation-safe disconnect cleanup. Test
  partial/multiple frames, malformed and oversized input, lost/partial writes, queue saturation,
  slow or nonreading peers, subscription cleanup, and zero leaked tasks on Linux and macOS.

  **Implementation plan**

  - Introduce the minimal current Tokio feature set for Unix networking, asynchronous read/write,
    bounded channels, task polling, and deterministic tests. Keep SQLite and application dispatch
    synchronous on their existing owners; this package moves only socket waits and bytes to Tokio.
  - Replace the public raw accepted stream with an opaque `AcceptedLocalStream` created only after
    foundation-owned kernel peer validation. Consume that capability when preparing session I/O so
    no caller can accidentally feed an unauthenticated socket into the production driver.
  - Define plain closed session events for decoded message, complete tracked write, and exactly one
    terminal close cause. Use one caller-owned bounded event channel and one fixed-capacity encoded
    write queue per connection; return explicit `Full`, `Closed`, invalid-message, or encode
    rejection without adding fallback queues.
  - Run bounded incremental frame decoding on the read half. Drain all complete retained frames
    before another read, stop reading while the event channel applies backpressure, classify EOF
    with retained bytes as truncated protocol input, and close on every malformed/oversized frame.
  - Run ordered complete-frame writes on the write half. Emit a `Written` ticket only after every
    byte of that exact frame succeeds; a write error, peer close, or cancellation may leave partial
    bytes but closes the stream and never retries or confirms the ticket.
  - Drive the read and write futures under one tracked per-session future with a separate close
    signal that cannot be blocked behind a saturated write queue. When either half terminates, stop
    the sibling half, drop the descriptor, emit one terminal event, and return with no child task.
  - Add deterministic duplex and real Unix-stream contracts for partial and multiple frames,
    malformed/oversized/truncated input, partial/lost writes, full/closed queues, ordered tickets,
    invalidation writes, slow/nonreading peers, prompt close, and exact terminal cleanup.

  **Risks and decisions**

  - Cancelling `write_all` can leave a partial frame. Cancellation always closes that connection;
    it never restarts the frame and never confirms the associated `WriteTicket`.
  - A shared unbounded event stream would move the memory problem upstream. The future central node
    loop supplies fixed event capacity, and each session stops socket reads while that bound is full.
  - Spawning detached reader and writer tasks complicates exact joining. One session future polls
    both owned halves concurrently and returns only after the descriptor and both loops are gone.
  - Raw Unix streams would bypass the peer-credential boundary. The opaque accepted-stream
    capability is constructible only by the foundation after kernel same-user validation.

  **Post-Plan Execution Steps**

  1. Add failing bounded read/write/event/close contracts before introducing Tokio.
  2. Implement the accepted-stream capability and one joined session I/O future.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before listener-loop
     coordination.

## 2026-08-27 — Bounded local session registry and dispatch

Added a fixed-capacity central registry that exclusively owns one transport-independent
`ServerSession`, bounded I/O handle, and joined byte task per authenticated connection. Admission
rejects closed intake, duplicate IDs, and full capacity before spawning. Dispatch borrows current
application and lifecycle capabilities per decoded message, queues exact responses, confirms only
matching completed tickets, and closes only the affected session on protocol, transport, queue, or
task failure. Post-write disposition is explicit, so final version rejection closes without a
state-inspection accessor.

Added bounded coalesced-invalidation delivery with deterministic slow-subscriber eviction and
registration cancellation. Explicit drain closes independently of queue capacity, consumes a
saturated shared event channel, joins every task, and reports zero retained sessions/tasks through
plain public report fields. Real authenticated Unix-stream contracts cover admission, negotiation
and request round trips, exact confirmations, response loss, event/write saturation, malformed-peer
sibling isolation, subscription cleanup, and complete drain. Shared application fakes remove test
duplication. Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/behavior/specification verifiers, four supported core and node targets,
both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Own bounded local session registry and dispatch** — Compose authenticated session
  I/O with one `ServerSession` per connection under a fixed-capacity central registry. Route decoded
  messages through call-scoped application/lifecycle capabilities, enqueue exact responses,
  confirm only matching completed tickets, deliver coalesced invalidations, disconnect saturated or
  failed sessions, and join every accepted I/O future. Test duplicate identities, capacity,
  negotiation/request round trips, response loss, write/event saturation, invalidation pressure,
  sibling isolation, disconnect cleanup, and zero retained sessions/tasks on Linux and macOS.

  **Implementation plan**

  - Define one registry configuration with explicit nonzero session, event, and per-session write
    capacities. Retain a bounded map from connection ID to `ServerSession` plus its sole I/O handle,
    and a `JoinSet` containing exactly one future for each admitted map entry.
  - Admit only opaque peer-validated streams. Reject capacity and duplicate IDs before spawning;
    prepare one I/O driver, reserve the map entry, spawn it, and roll back the entry if spawn setup
    cannot complete.
  - Route each decoded message against application and lifecycle capabilities borrowed only for the
    dispatch call. Enqueue the returned `OutboundMessage`; on full/closed/encode failure disconnect
    that session so its pending ticket and registrations are cancelled rather than silently lost.
  - Confirm a write only when the event's connection and ticket match that exact `ServerSession`.
    Treat stale/unknown completion, protocol-state failure, driver termination, or task panic as a
    session-local close without affecting siblings.
  - Poll each active session's coalesced invalidation at bounded safe points. If its fixed write
    queue cannot accept the notice, close the slow/nonreading session; reconnect performs a complete
    authoritative refresh, so no unbounded invalidation retry queue is introduced.
  - Provide explicit close-intake and drain operations. Close every session through the independent
    close signal, consume terminal events, cancel all revision registrations, join every task, and
    return a plain diagnostic report with zero retained entries.
  - Add deterministic multi-client contracts over real authenticated Unix streams for admission,
    dispatch, exact confirmations, response loss, duplicate/capacity rejection, slow-writer
    eviction, malformed disconnect, sibling survival, subscription cleanup, and complete drain.

  **Risks and decisions**

  - A map entry inserted after spawn could lose the only task handle on an intervening failure.
    Reserve all bounded state before spawn and keep task/map counts equal as an executable invariant.
  - Retrying a response after queue rejection would duplicate uncertain transport effects. Close
    the session; client replay policy remains responsible for mutation reconciliation.
  - Removing a coalesced invalidation before discovering a full write queue could lose a wake.
    Saturation closes the session, forcing the reconnecting client to refresh from authoritative
    state instead of retaining another queue.
  - One bad peer must not stop the node loop. Every protocol, queue, write, and join failure is
    scoped to its connection ID; only explicit registry drain closes siblings.

  **Post-Plan Execution Steps**

  1. Add failing registry capacity, dispatch, saturation, sibling, and drain contracts first.
  2. Implement central session ownership over existing protocol and byte-I/O machines.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before listener/signal
     coordination.

## 2026-08-27 — Owned listener and bounded local session pump

Added a one-time opaque transfer from the foundation-owned nonblocking Unix listener into Tokio
readiness while retaining device/inode pathname cleanup authority in the foundation. A shared
descriptor lease prevents cleanup from unlinking a still-live transferred socket. The central pump
fairly alternates ready listener and registry work without polling or a detached accept task,
kernel-validates every peer, derives checked boot-local connection IDs, rejects excess descriptors
before spawning, and retains closing capacity until exact task join.

Pump events and shutdown reports are plain public data. Application and lifecycle capabilities are
borrowed only during dispatch; methods remain for actual listener/session ownership operations.
Explicit intake closure drops listener readiness without disturbing live sessions, explicit
invalidation flush supports external revision wakes, and shutdown joins every session before
foundation cleanup. Real Unix-stream contracts cover once-only transfer, invalid nonce handling,
several clients, distinct IDs, accept pressure and fairness, full-capacity rejection, disconnect
reaping/re-admission, explicit close, zero retained tasks/sessions, cleanup, and immediate rebind.
Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/behavior/specification verifiers, four supported core and node targets,
both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Drive the owned listener and bounded local session pump** — Transfer the already
  bound foundation listener into one Tokio readiness owner and compose it with the bounded session
  registry under a sole call-scoped event pump. Generate collision-free boot-local connection IDs,
  accept and validate peers only when the descriptor is readable, reject excess sessions without
  spawning, route decoded work through borrowed capabilities, and surface bounded invalidation and
  session progress without a polling loop or detached accept task. Test multiple real clients,
  accept saturation, peer disconnect/re-admission, dispatch fairness, explicit intake closure, and
  complete listener/session drain on Linux and macOS.

  **Implementation plan**

  - Add a one-time foundation-private listener transfer that retains the socket artifact identity
    for conditional cleanup. The transferred capability remains opaque, performs kernel same-user
    validation on every accept, and is registered exactly once with Tokio readiness.
  - Define a pump configuration from the existing plain registry capacities plus a nonzero boot
    nonce. Derive unique nonzero connection IDs from that nonce and a checked monotonic counter;
    exhaustion closes intake rather than reusing an identity.
  - Select fairly between listener readiness and registry progress. Clear readiness on `WouldBlock`,
    admit only validated streams, and return plain progress events for accepted/rejected peers,
    decoded/write/task dispatch, and bounded invalidation delivery. Keep application and lifecycle
    capabilities borrowed only for the dispatch call.
  - Reject full/closed/colliding admission by dropping that accepted descriptor before any task is
    spawned. A closing entry retains its capacity until its sole task is joined, after which a new
    peer can be admitted without restarting the listener.
  - Close intake by dropping the transferred listener and closing registry admission. Drain consumes
    the pump, joins every session task, and returns plain listener/session diagnostics while the
    foundation continues to retain pathname cleanup ownership.
  - Add deterministic real Unix-stream contracts for several clients, full-capacity rejection,
    malformed/closed peer isolation, task reaping and re-admission, fair dispatch under listener
    pressure, explicit close, and zero retained session/task state.

  **Risks and decisions**

  - Repeated nonblocking polling would waste CPU and make signal fairness timing-dependent. Register
    the already nonblocking descriptor with Tokio readiness and retry only inside `try_io`.
  - Moving the descriptor must not move pathname cleanup authority. The foundation retains the
    device/inode identity and removes the socket only after the pump has dropped its listener.
  - Random per-accept identity generation introduces an unnecessary entropy failure path. One fresh
    nonzero boot nonce plus a checked counter is unique for the process generation and remains
    diagnostic, never authoritative.

  **Post-Plan Execution Steps**

  1. Add failing listener transfer, saturation, fairness, re-admission, and drain contracts first.
  2. Implement the Tokio listener capability and central pump over the existing registry.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before lifecycle and
     signal coordination.

## 2026-08-27 — Ordered lifecycle, signal, and graceful node drain

Composed the ready `NodeOwner` and bounded local session pump under one asynchronous runtime owner.
The runtime binds and publishes readiness before the one-time listener transfer, projects typed
status/readiness from node-owned state, and records stop/restart intent in a call-scoped cell before
the sole event loop performs the lifecycle mutation. No owner mutex or DTO accessor boilerplate was
introduced: runtime configuration and complete shutdown diagnostics are plain public data, while
methods remain limited to ownership and state transitions.

The registry now distinguishes connection admission from decoded-request intake and tracks each
accepted response until its exact write confirmation or session loss. Drain closes listener and
request intake first, preserves an accepted lifecycle acknowledgement behind a fixed deadline, then
joins every local I/O task before ordered component/task/foundation cleanup. Queued decoded events
take precedence over a completed transport join, preventing immediate peer EOF from erasing an
already-decoded restart request. `SIGINT` and `SIGTERM` share the same stop path, restart remains an
intent for the later coordinator, and cleanup issues accumulate without preventing artifact or
state-lock release.

Real authenticated-client contracts cover ready status, delivered stop acknowledgement, lost
restart acknowledgement, connected-client EOF, repeated/conflicting lifecycle intent, external
shutdown selection, Unix signal registration, cleanup failure accumulation, zero retained
sessions/tasks, artifact removal, state-lock reacquisition, immediate listener rebind, and invalid
zero timeout. Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/behavior/specification verifiers, four supported core and node target
checks, both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Coordinate lifecycle requests, Unix signals, and graceful node drain** — Compose the
  local session pump with the sole `NodeOwner`, implement typed status/readiness/stop/restart control,
  and route `SIGINT`/`SIGTERM` plus protocol lifecycle intent through one ordered asynchronous drain.
  Preserve already-accepted lifecycle acknowledgements before closing clients, then join all local
  I/O, drain components/tasks, clean readiness/socket artifacts, and return explicit stop versus
  restart intent. Test signals, repeated stop/restart, lost lifecycle acknowledgements, connected
  client shutdown, cleanup failures, and immediate restart/rebind on Linux and macOS.

  **Implementation plan**

  - Add a complete runtime configuration from the existing pump configuration, safe build metadata,
    authority policy, and a nonzero accepted-response drain timeout. Start from a ready `NodeOwner`:
    bind the listener, publish readiness with the same boot nonce, and transfer the listener into the
    pump. Any partial startup failure drops the owner and cleans its exact artifacts.
  - Implement a call-scoped lifecycle capability from an immutable status snapshot plus a one-call
    intent sink. Status/readiness return the current typed phase, build, and durable revision. Stop or
    restart returns `Draining` and records one intent; after dispatch returns, the sole runtime mutates
    `NodeOwner` before processing another event. Do not add interior mutability to node ownership.
  - Track response-write admission in the session registry independently of protocol internals.
    When drain starts, close listener and decoded-request intake, then continue only terminal and
    exact write-completion progress until every response accepted before drain is written or its
    session closes. A fixed timeout closes remaining sessions and is recorded in the plain report.
  - Select fairly between pump progress and one fused shutdown future. Protocol stop/restart keeps
    its exact response in the accepted-write drain; `SIGINT` and `SIGTERM` map to stop without a
    protocol acknowledgement. Repeated lifecycle transition calls remain idempotent and conflicting
    stop/restart intent fails closed.
  - After accepted response drain, close/join every local session task before invoking the existing
    synchronous ordered component/task/foundation shutdown. Return plain intent, timeout, local
    session, and node cleanup diagnostics; restart means only clean release for a later coordinator.
  - Add deterministic runtime contracts with real authenticated clients and scripted shutdown
    futures for status/readiness, stop/restart acknowledgement success and loss, accepted response
    drain, nonreading timeout, connected-client close, repeated intent, component cleanup issues,
    zero retained tasks, immediate state-lock reacquisition, and listener rebind. Compile and smoke
    the real Unix signal registration on Linux and macOS without signaling the shared test process.

  **Risks and decisions**

  - `LifecycleControl` uses `&self`; hiding the owner behind a mutex would blur the sole event-loop
    mutation boundary. Record intent in a call-local cell and apply it immediately after synchronous
    request dispatch, before any next event.
  - Closing sessions immediately can discard the very stop/restart response that tells a client its
    request was accepted. Close request intake first, retain exact already-queued writes, and close
    descriptors only after completion, loss, or the explicit deadline.
  - Waiting without a deadline lets one nonreading peer prevent signals and ownership release. Use
    one fixed runtime-configured deadline and report timeout rather than silently claiming delivery.
  - Restart is intent, not process spawning. This package returns `Restart` only after complete
    cleanup; the following client/CLI coordinator owns replacement startup and convergence.

  **Post-Plan Execution Steps**

  1. Add failing lifecycle, accepted-write drain, timeout, signal-selection, and cleanup contracts.
  2. Implement response-intake closure and the sole runtime coordinator over `NodeOwner` and pump.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before autostart/CLI.

## 2026-08-27 — Convergent node autostart and lifecycle CLI

Added a bounded one-shot lifecycle client that opens a fresh private Unix connection, negotiates
local API v1, issues exactly one typed lifecycle request, and strictly decodes its correlated
response. It distinguishes absent, incompatible, protocol, transport, response-loss, and readiness
failures. Readiness metadata is read only after a successful protocol handshake and remains a
diagnostic generation record rather than liveness or ownership authority.

Implemented one protocol-first client coordinator over injected probe and child-launch seams.
Already-ready owners return without spawning; absent owners start one candidate; concurrent callers
may race children but converge on the exclusive state-lock winner. Candidate exit is retained until
the fixed readiness deadline so a losing child cannot mask another winner. Stop converges
idempotently to absence, while restart tolerates acknowledgement loss and returns only after a
distinct boot nonce negotiates `Ready`. The real launcher passes arguments without a shell, closes
standard streams, and assigns children to an explicit waiter.

The single Rust `hq` executable now supports explicit `node run`, `status`, `readiness`, `stop`, and
`restart` roles with optional absolute state roots. The foreground process composes the available
foundation and dormant future-component owners, uses a fresh boot nonce per generation, drains via
signals or protocol, and reacquires every owner for same-process restart. Configs, observations,
outcomes, and diagnostics use public fields; methods are limited to protocol, launch, ownership, and
convergence operations.

Scripted tests cover live/absent/incompatible peers, losing child exit, deadline failure, lost stop
and restart acknowledgements, and distinct-generation convergence. Real process tests cover bounded
probe/stop, concurrent autostart callers, state-lock selection, machine-stable status/readiness,
foreground restart, old connected-socket closure, fresh reconnect, stop, process exit, and complete
artifact cleanup. Full locked workspace format/check/build/tests/doctests/Clippy,
architecture/dependency/behavior/specification verifiers, four supported core and node target
checks, both 512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[node/high] Implement convergent autostart and lifecycle CLI roles** — Add one client coordinator
  that probes the owned socket, starts the foreground-node child only when absent, waits on typed
  readiness, and converges concurrent launchers on one owner without PID-file authority. Wire
  explicit foreground run, status, readiness, stop, and restart roles through the single `hq`
  executable with actionable phase/path/cause/action diagnostics. Test absent/stale/live nodes,
  concurrent starters, child failure, readiness timeout, lost lifecycle acknowledgements,
  connected-client reconnect after restart, and runtime artifact cleanup on Linux and macOS.

  **Implementation plan**

  - Define a state-root-qualified client configuration with the existing runtime paths, build
    metadata, positive connect/readiness timeouts, and bounded retry cadence. Runtime discovery must
    not load the root signer: the first Rust CLI uses the private state-local runtime directory, and
    readiness/PID fields remain diagnostics after a successful same-user protocol connection.
  - Add one blocking, bounded lifecycle probe over the private Unix socket. Perform the v1 hello,
    send exactly one status/readiness/stop/restart request, strictly decode one complete bounded
    response, and return plain typed outcomes for absent, incompatible, lost, or successful peers.
    Never infer liveness from the readiness file or process ID.
  - Build a coordinator over an injected child launcher, monotonic deadline, and sleeper. Probe
    first; spawn `hq node run --state-root ...` only when no protocol owner responds; then wait for
    any compatible owner. Concurrent callers may both spawn, but the state lock selects one child
    and every caller converges on its protocol readiness. Child failure wins only if no peer owner
    appears before the deadline.
  - Implement stop as an idempotent terminal convergence and restart as generation convergence.
    Retain the previous readiness boot nonce only as a diagnostic generation marker, tolerate a lost
    lifecycle response, and require the old socket generation to disappear plus a newly negotiated
    `Ready` generation before reporting restart success.
  - Add a foreground generation coordinator using minimal dormant component owners for capabilities
    not implemented by later packages. Open the foundation, create the graceful local runtime with a
    fresh nonzero boot nonce, run through Unix signals, and on `Restart` reacquire every owner and
    start a fresh generation in the same foreground child; `Stop` exits only after complete cleanup.
  - Parse only explicit `hq node run|status|readiness|stop|restart` roles plus optional absolute
    `--state-root`. Keep stdout machine-stable and diagnostics redacted/actionable. The binary main
    maps typed path, identity, startup, child, timeout, protocol, and cleanup failures to one bounded
    human message and a nonzero exit status.
  - Add deterministic fakes plus real child-process contracts for live/absent/stale probes,
    concurrent launchers, loser-child convergence, early child failure, deadline expiry, lifecycle
    response loss, stop idempotency, restart generation change, a connected client's reconnect,
    foreground signal/stop cleanup, state-lock release, and immediate socket rebind on Linux/macOS.

  **Risks and decisions**

  - Reading private identity bytes merely to derive an XDG socket path would violate the client
    trust boundary. Keep this first CLI runtime namespace under the explicit state root; later path
    migration must be a versioned public locator, not secret-file inspection.
  - A spawn mutex or PID file would create a second authority. Multiple children may race; only the
    existing exclusive state lock grants ownership, and clients accept only a negotiated same-user
    socket peer.
  - A child can fail because another launcher won. Do not surface that exit while a compatible peer
    becomes ready; retain it only as a diagnostic if the bounded readiness deadline expires.
  - Restart acknowledgement loss is uncertain, not automatic failure. Observe protocol generation
    convergence: old ownership must release and a fresh boot nonce must negotiate `Ready`.
  - Detached child ownership must not leak zombies. The real launcher uses null standard streams and
    a bounded reaper thread; tests inject child handles and never signal unrelated processes.

  **Post-Plan Execution Steps**

  1. Add failing one-shot probe, fake-launch convergence, foreground restart, and CLI parsing tests.
  2. Implement the client coordinator, real child launcher, dormant components, foreground loop,
     and single-binary roles.
  3. Run every Rust, target, fuzz, dependency, whitespace, and unchanged-Go gate.
  4. Commit conventionally, archive this exact entry with evidence, and amend before relay transport.

## 2026-08-27 — Encrypted Nostr envelope v1

Specified the Rust transport protocol independently from canonical v1 and pinned the reviewed
NIP-44, NIP-59, NIP-42, and standard-vector revisions. The specification fixes the schema-1 HQ
rumor, kind-13 seal, retained kind-1059 gift wrap, relay-visible metadata, trust transitions,
redacted failure classes, allocation limits, quarantine evidence limit, timestamp window, exact
retry lineage, one-use-key uniqueness claim, and the rule that transport observations never grant
domain authority. It also deliberately drops the old Go policy of persisting an unnecessary
one-use secret after the wrapper has been fully signed.

Implemented bounded NIP-44 v2 with normalized x-only secp256k1 ECDH, HKDF-SHA256, ChaCha20,
HMAC-SHA256, constant-time MAC comparison, current short and extended length prefixes, strict
base64/version/padding checks, early allocation bounds, and derived-key zeroization. Added strict
NIP-01 transport event construction and verification, schema-1 preparation/opening, recipient and
sender/origin/canonical agreement, independently randomized prior-two-day timestamps, immutable
exact publish bytes, durable relay-visible metadata restoration, one-use collision checks, and
bounded signed NIP-42 inputs under `hq-relay::envelope::v1`. Passive configs and reports use public
fields; only root-key ownership and immutable retry bytes remain opaque.

Tests reproduce the published NIP-44 vector, round-trip the extended-length form and exact embedded
canonical bytes, detect ciphertext and outer-event tampering, reject wrong recipients and signer
mismatches, prove fresh one-use keys and byte-identical retries, verify persistence metadata,
timestamp bounds, collision behavior, oversize input, and exact NIP-42 tags. Full locked workspace
format/check/tests/doctests/strict-Clippy, architecture/dependency/behavior/specification verifiers,
four supported relay target checks, both 512-run fuzz smokes, whitespace checks, and unchanged Go
vet/build/fresh tests pass.

### Original plan entry

- **[transport/high] Specify and implement the encrypted Nostr envelope** — Write Nostr envelope v1
  independently from canonical v1, then implement recipient binding, NIP-44 encryption, NIP-59
  wrapping, NIP-42 authentication inputs, identity agreement, randomized transport timestamps,
  exact durable wrapper creation before first publish, and exact-byte reuse within a retry lineage.
  Define relay-visible data and input/quarantine bounds. Add standard vectors and tamper,
  wrong-recipient, signer mismatch, key reuse, retry, and size tests. Complete this work when opened
  envelopes yield only raw canonical bytes for the common verification/ingest path and transport
  metadata cannot grant domain authority.

  Implementation plan:
  - Specify schema-1 envelope bytes, independently pinned NIP-44/59/42 rules, relay-visible
    metadata, trust transitions, size limits, redacted failure classes, and bounded quarantine
    evidence in `docs/protocol/nostr-envelope-v1.md`.
  - Add `hq-relay::nip44` from the published standard vectors: normalized x-only secp256k1 ECDH,
    HKDF-SHA256, ChaCha20, HMAC-SHA256, current short/extended length padding, strict base64/version
    and allocation preflight, constant-time MAC verification, and zeroization of derived keys.
  - Add strict NIP-01 transport-event encoding and verification, then schema-1 rumor, signed kind-13
    seal, and signed kind-1059 gift-wrap preparation/opening with exact recipient, signer, canonical
    ID, installation ID, and public-key agreement.
  - Model a prepared retry lineage as an owned validated exact wrapper plus public persistence
    metadata. Publishing can only borrow those retained bytes; reconstructing durable state verifies
    all relay-visible metadata and wrapper integrity, and a separate one-use-key claim detects
    cross-lineage reuse.
  - Produce typed NIP-42 kind-22242 signing inputs without giving relay challenges or URLs domain
    authority. Cover official vectors, round trips, tampering at every layer, wrong recipients,
    signer/origin mismatches, fresh and reused one-use keys, exact retries, timestamp windows,
    malformed/oversize input, and bounded quarantine evidence.

  Risks and follow-ups:
  - NIP-44's current extended-length form is newer than the frozen Go implementation; pin the
    reviewed upstream revisions and test both short and extended lengths so the Rust protocol is
    intentional rather than accidental compatibility.
  - Exact wrapper reuse is a storage/session obligation as well as a codec invariant. This package
    defines validated persistence/retry objects; the following durable relay package must enforce
    the one-use public-key uniqueness claim transactionally before first publish.
  - NIP-59 hides the real signer only inside the outer encryption and NIP-42 reveals the root key to
    a relay. Document both observations and retain no plaintext or secret material in quarantine.

## 2026-08-27 — Durable relay synchronization state

Specified relay synchronization v1 with explicit ownership, exact retry lineage, monotonic
attempt and cursor rules, dual deduplication, bounded staging and quarantine, generation changes,
shutdown obligations, and the invariant that relays never grant canonical authority. Added one
shared validated `RelayUrl` and consumer-owned relay ports whose passive records use idiomatic
public fields; only values protecting validation or exact-byte invariants remain opaque.

Implemented clean-sheet storage schema v10 with storage-owned records and strict transactional
tables for policy operations and generations, prepared wrappers and one-use-key claims, attempts,
cursors, inbound claims, staging, and quarantine. Equal retries are idempotent, unequal identity
reuse conflicts, preparation and uniqueness claims commit together, monotonic transitions fail
closed, successful or permanently rejected staged input is removed atomically, staging applies
backpressure at its inclusive bounds, and quarantine evicts deterministically. State survives
restart and is excluded from projection repair. A narrow cloneable store handle lets future relay
session owners share requests without sharing store shutdown ownership.

Added the node-only adapter between relay and storage vocabularies, including strict URL, code,
generation, and prepared-envelope revalidation. Replaced the duplicate node URL type, updated
storage and architecture documentation, and strengthened dependency checks so transport and SQLite
records cannot leak across their boundary. Contracts cover public fields, URL bounds, stable policy
operations, collisions, uncertain attempt recovery, cursor regression, dual claims, atomic staged
transitions, FIFO/backpressure, deterministic quarantine eviction, corruption, restart, and repair.
All locked workspace format/check/build/tests/doctests/strict-Clippy gates, architecture/dependency/
behavior/specification verifiers, four supported portable targets including `hq-relay`, both
512-run fuzz smokes, whitespace checks, and unchanged Go vet/build/fresh tests pass.

### Original plan entry

- **[transport/high] Specify and persist durable relay synchronization state** — Define the complete
  relay synchronization state machine and its consumer-owned ports, then extend clean-sheet storage
  for versioned relay policies, prepared exact wrappers and one-use-key claims, per-relay attempt and
  acceptance state, overlapping catch-up cursors, outer/logical deduplication, bounded staging, and
  bounded quarantine. Keep relay DTOs out of SQLite and SQLite DTOs out of `hq-relay`; map them only
  at the node composition boundary. Complete this package when every durable transition is bounded,
  transactional, collision-checked, survives restart, and is unchanged by explicit projection
  repair.

  **Implementation plan**

  - Create `docs/protocol/relay-sync-v1.md` with ownership, state transitions, exact retry and
    relay-acceptance semantics, overlap cursor rules, deduplication identities, generation changes,
    staging/quarantine bounds and eviction, shutdown obligations, and non-authority rules.
  - Add `crates/hq-relay/src/url.rs` for one validated WebSocket `RelayUrl`; replace the duplicate
    node-local relay endpoint type. Add `crates/hq-relay/src/ports.rs` with public-field configuration
    and observation records plus consumer-owned persistence, route-resolution, canonical-ingest,
    clock, and connection traits. Keep validated identities/exact-byte lineages opaque only where a
    mutation would break an invariant.
  - Extend `crates/hq-store/src/operational.rs`, `database/operational.rs`, `database.rs`, `actor.rs`,
    `error.rs`, and `lib.rs` with storage-owned relay records and bounded actor requests. Bump the
    clean-sheet schema/marker and add strict tables/constraints for configuration generations,
    prepared wrappers, one-use public keys, attempts, acceptance, cursors, inbound identities,
    staging, and quarantine; do not add a migration from old non-empty schemas.
  - Add the node-only mapping adapter and update `crates/hq-node/src/identity/config.rs` and exports
    so unsigned local defaults and durable policy use the same validated relay URL without letting
    local configuration bypass the durable generation/operation boundary.
  - Write failing URL/port tests and store contracts first: inclusive bounds; equal replay versus
    changed-value collision; atomic prepare plus one-use claim; response-loss attempt recovery;
    acceptance monotonicity; overlap cursor regression; outer and logical duplicate idempotency;
    FIFO staging; deterministic quarantine count/byte eviction; corruption rejection; restart and
    projection-repair preservation; and public fields on passive relay records.
  - Update `docs/rust/storage.md`, architecture/dependency checks, and schema contract expectations;
    then run every locked workspace, supported-target, fuzz, whitespace, and unchanged-Go gate.

  **Risks and decisions**

  - Existing outbox intents lack routes and encryption keys. This package stores no inferred route;
    the later session owner resolves signed routing state through a read port immediately before
    first preparation, and relay observations can never write that route.
  - Exact preparation and a one-use-key claim must commit together. A crash may leave an intent
    queued or a complete prepared lineage, never bytes without their uniqueness claim.
  - Staging contains retryable exact outer bytes; quarantine retains only bounded raw outer evidence
    and redacted classification, never decrypted plaintext or secrets. Count and total-byte bounds
    are enforced transactionally before commit.
  - Schema v9 has no production Rust data yet and the rewrite explicitly rejects migration. Bump the
    clean-sheet identity and keep non-empty older databases incompatible rather than inventing a
    transitional protocol.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-27 — Owned relay sessions and deterministic synchronization

Added deterministic per-relay session ownership and a coalescing manager over consumer-owned ports, with live-before-retained catch-up, bounded buffering, NIP-42 authentication, byte-identical durable publishing, closed acknowledgement classes, staged recovery, quarantine, generation refresh, and joined drain. Relay state now uses independent typed keyset pagination and storage schema v11 so work beyond the first bounded page remains reachable. Scripted contracts cover response/send loss, restart, equal-time catch-up stalls, transient and permanent input failures, auth challenge replacement, route exclusion, healthy-socket preservation, overflow, policy refresh, wake coalescing, and shutdown; all workspace, target, fuzz, dependency, shell, and unchanged-Go gates pass.

### Original plan entry

- **[transport/high] Implement owned relay sessions and deterministic synchronization** — Implement
  one state-machine owner per configured relay over the durable ports: connection/authentication,
  live subscription before retained backward pagination with overlap, live-edge buffering,
  outbound exact wrapper preparation and byte-identical retry, positive/duplicate/negative `OK`,
  disconnect and response-loss recovery, bounded exponential backoff, configuration refresh,
  coalesced work wakes, staging retry, quarantine, and graceful drain. Use a deterministic scripted
  relay for EOSE, duplicates, auth, rejection, missed wake, reconnect, restart, and shutdown. Complete
  this package when healthy sessions survive ordinary wakes and every scripted failure converges to
  the documented durable state without relay metadata reaching canonical reduction.

  **Implementation plan**

  - Add keyset continuation records to the relay-state query port and storage adapter before session
    work. The previous bounded snapshot has no way to reach work after the first 1,024 durable rows;
    page every independently ordered collection without exposing SQLite records to `hq-relay`.
    Extend attempt records with a closed redacted rejection class so durable negative `OK` state
    matches the normative protocol rather than retaining relay prose.
  - Refine the connection and envelope seams for an exclusively owned blocking session: bounded
    receive polling distinguishes timeout from closure, and an injected envelope capability prepares
    immutable outbound wrappers, opens inbound wrappers, identifies the local recipient, and signs
    connection-local NIP-42 challenges. Keep passive request/result fields public and secrets plus
    exact retry lineages opaque.
  - Implement the relay session as a deterministic state machine over one policy generation. Open
    the live subscription before retained catch-up, buffer the live edge within explicit count/byte
    bounds, page retained input backward with inclusive overlap, advance only on strictly older
    `(created_at, wrapper ID)` boundaries, and never claim exhaustion from a repeated full page.
    Process all input through open, common canonical ingest, atomic dual claim, staging, or bounded
    quarantine transitions without passing relay metadata to the ingest port.
  - Implement outbound work from durable state only: resolve signed routes immediately before first
    preparation, commit exact preparation before sending, publish only to an eligible writable
    policy, persist uncertainty before every write, correlate `OK` by wrapper ID, treat positive and
    duplicate acknowledgements as accepted, retain closed negative classifications, authenticate and
    retry `auth-required`, and use capped exponential retry with injected time and deterministic
    jitter. Never regenerate prepared bytes.
  - Add one manager owner with a bounded coalescing wake, periodic missed-wake repair, exactly one
    worker per enabled URL, generation-aware refresh, and ordered graceful drain. Ordinary work wakes
    keep healthy connections and subscriptions; relevant policy changes replace only that owner.
    Shutdown closes intake, relies on pre-send durable uncertainty, closes named subscriptions and
    connections, joins every worker, and reports bounded stable causes without relay prose.
  - Build a deterministic scripted connector/store/clock/envelope harness. Cover live-before-catch-up,
    overlap and equal-time stalls, live buffering and staging overflow, duplicates, EOSE, auth
    challenge replacement and `auth-required`, positive/duplicate/negative `OK`, send and response
    loss, exact retry after restart, route exclusion, capped backoff, missed/coalesced wakes, policy
    refresh, staging recovery, quarantine, disabled/read-only/write-only policies, and complete drain.
    Update the relay specification and storage/architecture documentation, then run every locked
    workspace, supported-target, fuzz, dependency, whitespace, and unchanged-Go gate.

  **Risks and decisions**

  - NIP-01 has no portable event-ID range filter. An inclusive full page that makes no strictly older
    progress must remain unexhausted and retry rather than skipping an unbounded same-second tie; the
    controlled interoperability package will measure this fail-safe behavior against real relays.
  - The relay connection seam must permit bounded receive polling so work wakes, policy refresh, and
    shutdown cannot be held hostage by a silent socket. The future WebSocket adapter owns timeout
    mechanics; the session owns the deadline and interpretation.
  - A negative relay response is transport-local. Store only a closed redacted class and retry
    deadline; never persist free-form relay text or translate rejection into canonical authority.
  - Exact inbound ingest and its deduplication claim are separate idempotent transactions. A crash
    between them may leave a verified canonical fact without a claim, but exact replay re-verifies
    idempotently and completes the dual claim without changing domain meaning.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-27 — Two-replica relay convergence and controlled interoperability

Composed the production relay manager into the foreground Rust node with verified authority-route
resolution, secret-owning envelope construction, the common reverified canonical-ingest path, and
restricted store capabilities instead of exposing store ownership. Added a bounded
Tungstenite/Rustls NIP-01/NIP-42 adapter; durable schema-v12 catch-up coverage that refreshes across
arbitrary downtime; direct grant/revoke fanout; and an opt-in pinned-rnostr smoke with actionable
preflight. A deterministic two-installation retained-relay test now proves convergence through
shuffled pages, duplicates, downtime, relay/client restarts, uncertain acknowledgement, and route
revoke/regrant while transport observations remain non-authoritative. Locked workspace, strict
Clippy, documentation, architecture/spec/dependency, four portable-target relay, both 512-run fuzz,
shell, whitespace, and unchanged-Go gates pass. The controlled smoke was recorded separately as not
run because the installed Docker daemon was unavailable, which remains explicitly non-gating.

### Original plan entry

- **[transport/high] Prove two-replica convergence and controlled relay interoperability** — Compose
  the real node relay manager, storage adapter, route resolution, root envelope identity, and common
  canonical ingest path. Prove two distinct Rust installations converge across arbitrary delivery
  order, duplication, downtime, offline retained catch-up, relay restart, client restart, revoke and
  regrant traffic, and uncertain publish responses. Add an opt-in controlled real-retained-relay
  smoke covering NIP-42 and catch-up without making external availability a unit gate. Complete this
  package when direct state/reducer evidence proves relay order and observations cannot influence
  the converged result.

## 2026-08-27 — Provider-neutral harness contract and reusable conformance

Defined the synchronous object-safe managed-runtime boundary with passive public capability,
request, result, output, activity, and shutdown records; typed stable failures; exact durable-session
readiness; identity-and-digest submission reconciliation; structured non-secret interaction; and
explicit cancellation and teardown. Added a provider registry that rejects duplicate or unsafe
recovery declarations and cleans up mismatched resumes. A reusable 14-scenario conformance driver
and deterministic scripted adapter now prove new/resumed/missing sessions, response loss,
lookup-before-retry, active-operation races, collisions, request handling, output order, crash
isolation, and teardown. Normative specifications, architecture enforcement, and the four-target CI
matrix keep provider vocabulary and runtime/process/serialization concerns outside the neutral
crate. Locked workspace, strict Clippy, documentation, architecture/spec/dependency, four portable
target, both 512-run fuzz, shell, whitespace, and unchanged-Go gates pass.

### Original plan entry

- **[harness/high] Define the provider-neutral harness contract and conformance suite** — Specify
  logical instances, durable sessions, capabilities, start/resume readiness, stable submission IDs,
  accepted/rejected/uncertain outcomes, lookup/reconciliation requirements, interactive requests,
  normalized output/activity, cancellation, and shutdown. Implement neutral traits and a scripted
  fake provider; registration must reject adapters lacking safe idempotency or reconciliation.
  Ensure neutral crates contain no Codex vocabulary. Complete this work when the fake passes a
  reusable conformance suite covering new/resumed sessions, response loss, active-operation races,
  interactive requests, output, crash isolation, and teardown.

## 2026-08-27 — Harness supervisor ownership and durable recovery

Implemented a synchronous provider-neutral supervisor with exact-token expiring ownership per
named agent, automatic resume repair, exact delivery replay, terminal accepted/rejected state,
lookup-before-retry reconciliation, structured cancellation, bounded FIFO/tail coalescing,
output-before-activity checkpointing, stable persistence collision detection, copied/redacted
memory-only environments, and ordered bounded shutdown. Storage schema v13 now durably owns leases,
ready sessions, exact deliveries, and partial event checkpoints through a restricted handle; the
node owns the record mapping and concrete lifecycle/application component. Deterministic tests cover
restart after accepted-response loss, live lease competition and expiry, terminal replay, bounded
runnable scans, concurrent agents, saturation/coalescing, partial persistence, collision, secret
exclusion, intake closure, drain escalation, forced termination, exact release, repair survival, and
reopen recovery. Passive records expose public fields; only invariant-bearing owner tokens and
secret environment values remain opaque. Locked workspace, strict Clippy, documentation,
architecture/spec/dependency, four portable-target, both 512-run fuzz, whitespace, and unchanged-Go
gates pass.

### Original plan entry

- **[harness/high] Implement supervisor ownership, delivery recovery, and bounded persistence** —
  Implement one logical worker owner per named agent, durable ownership and delivery ledgers,
  pending/uncertain/accepted reconciliation, automatic wake from durable pending work, bounded FIFO
  plus keyed coalescing, output-before-activity persistence, stable output collision checks,
  environment-copy/redaction policy, and stop-intake/drain/escalate shutdown. Test daemon restart,
  lease races, response loss, buffer saturation, coalescing order, partial output/activity commits,
  concurrent agents, secret exclusion, drain timeout, and forced process termination with the fake
  adapter. Complete this work when accepted work is never silently lost or duplicated.

## 2026-08-27 — Path-resource identity, health, conflict, and release assessment

Added the outward `hq-resources` adapter with home-qualified display/canonical path identity,
nearest-existing-ancestor reservation, symlink revalidation, closed health conditions,
component-aware conflict reports, deterministic primary and launch policy, and bounded Git release
assessment. Passive requests and results expose public fields while filesystem/Git capabilities
remain opaque. Canonical facts, reducer aggregates, v13 clean-sheet storage, application snapshots,
and local API DTOs now retain both display and canonical locators; atomic replacement and advisory
claim recovery remain one canonical transition. Deterministic fake and real filesystem/Git tests
cover missing/inaccessible/malformed paths, retargeted links, linked worktrees, every dirty class,
unknown and forced release, malformed output, and hard subprocess deadlines. Locked workspace,
strict Clippy, architecture, dependency, four-target, fuzz, whitespace, and unchanged-Go gates pass
without bumping the unshipped storage version.

### Original plan entry

- **[resources/high] Implement path-resource identity, conflict, health, and release assessment** —
  Implement home-qualified absolute path locators, human spelling versus canonical identity,
  nearest-existing-ancestor handling for missing paths, symlink revalidation, equal/ancestor/
  descendant conflict detection, project-local overlap, resource health, Git cleanliness, primary
  path selection, launch-directory validation, and advisory claim persistence. Keep filesystem/Git
  observations outside pure project policy and never silently relocate or delete resources. Test
  missing/inaccessible paths, symlinks, worktrees sharing a Git directory, dirty/unknown release,
  atomic replacement, and explicit force behavior. Complete this work when every path decision is
  deterministic, explainable, and auditable.

## 2026-08-27 — Project command and durable saga foundations

Implemented the `hq-projects` outward workflow foundation with public passive application
commands, results, and checkpoints; a strict canonical v1 remote-command codec; exact-replay
intake; and bounded recovery. Added non-rebuildable project saga and destination-reservation
records to the clean-sheet storage v13 schema in place without a version bump, including project
cardinality, monotonic transitions, exact typed effect outcomes, repair/reopen survival, and a
node-owned store adapter. Replaced accessor-only passive effect and session records with idiomatic
public Rust fields and extended architecture, storage, CI, and restart contract evidence. Locked
workspace, strict Clippy, all-target test/build, four portable-target, architecture, dependency,
specification, fuzz, whitespace, and unchanged-Go gates pass.

### Original plan entry

- **[projects/high] Establish project command and durable saga foundations** — Define the complete
  typed project command/result vocabulary, strict versioned remote-command body codec, explicit
  workflow checkpoints, exact-replay intake, one-unresolved-command project serialization, and
  non-rebuildable SQLite checkpoint/reservation state. Keep passive Rust records public and
  behavior-owning capabilities opaque. Update the clean-sheet storage v13 schema in place without a
  version bump. Complete this work when changed identity, monotonicity, bounded recovery,
  repair-survival, restart, codec, and composition-adapter contracts pass.

  **Implementation plan**

  - Add an outward `hq-projects` crate for explicit project workflows. It may depend inward on the
    domain, reducer snapshots, application ports, neutral harness contract, and read-only resource
    adapter; none of those crates may import it. Keep each workflow as its own closed command/stage
    transition rather than introducing a generic workflow engine. Passive commands, checkpoints,
    reports, conflicts, and recovery records expose public fields. Managers and injected effect
    capabilities remain opaque because they own serialization, bounded work, and exact retry.
  - Define one typed application project command family with stable operation ID, exact
    digest, account/project/home identity, expected project head, and closed actions for open,
    activation, dispatch repair, graceful/forced close, archive, handoff/takeover, retirement,
    resource add/remove/replace, and worktree provisioning. Expose typed accepted/running,
    completed, rejected, and reconcilable-unknown outcomes. One unresolved state-changing command
    per project is enforced durably; local protocol projection belongs to the workflow package that
    can expose authoritative progress rather than a placeholder transport shape.
  - Persist non-rebuildable workflow state in the existing unshipped clean-sheet storage v13 schema
    without a version bump. Store exact command/digest, project/home, expected head, closed workflow
    kind and stage, derived external operation identities, reservation identity,
    definite/unknown effect result, and terminal outcome. Add exact collision,
    monotonic-stage, project-cardinality, bounded scan, atomic mutation, repair-survival,
    and reopen validation. Canonical project facts remain the sole durable project authority; saga
    rows are coordination checkpoints and never override projections.
  - Encode command behavior with one strict canonical versioned codec; never parse free-form human
    or diagnostic text. Map the outward workflow store capability to store-owned records in
    `hq-node` without reversing dependencies. Build test-first contracts for public passive values,
    exact and changed replay, project cardinality, monotonic stages/effects, bounded scans,
    reservation conflicts, repair survival, close/reopen recovery, and strict codec rejection. Run
    every locked workspace, supported-target, dependency, shell, whitespace, and unchanged-Go gate.

  **Risks and decisions**

  - Canonical facts and projections are authority; workflow checkpoints only answer what external
    work may have happened and what must be reconciled next. Recovery always rereads both before an
    effect, so a lost checkpoint response cannot regress or duplicate canonical state.
  - Remote command bodies must be strict versioned structured data even though the reducer retains
    them as inert bounded content. Only the workflow codec may decode that namespace, and canonical
    digest agreement is checked before execution. Unknown versions are definite rejections, never
    best-effort behavior or human-text parsing.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement
  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.


## 2026-08-27 — Project activation and at-most-once dispatch

Implemented transaction-consistent canonical project mutations plus a durable activation,
compensation, and ordered dispatch workflow. Configuring assignments now carry public session-free
intent and bind an acknowledged provider session only at runnable readiness. Every canonical
compare-and-swap is checkpointed exactly before commit for restart-safe reconciliation; resource,
runtime, launch, delivery, and compensation uncertainty remain bounded and replayable. Pending
inputs drain separately in authoritative sequence through the harness supervisor's sole durable
ledger, with complete attribution, changed-input collision rejection, and dispatch facts only after
definite acceptance. Storage v13 was updated in place without a migration or version bump. Full
workspace, strict Clippy, four-target, architecture, dependency, specification, fuzz, whitespace,
and unchanged-Go gates pass.

### Original plan entry

- **[projects/high] Implement activation and at-most-once project dispatch** — Add the
  transaction-consistent canonical project mutation capability and explicit activation workflow:
  expected-head/home/active-human validation, resource observation and claim preview, conditional
  open, configuring assignment, project-bound start or exact resume, launch-directory validation,
  thread selection from the first pending project message or explicit historical resume, runnable
  transition, and compensation to the documented prior stable state. Drain accepted inputs in home
  sequence through the harness supervisor's sole durable delivery ledger, reconcile before retry,
  and author dispatch only after definite acceptance. Test every crash and definite/unknown failure
  boundary, stale heads, claim/agent conflicts, launch failure, pending-message preservation,
  accepted-response loss, changed input, restart repair, late output, and complete attribution.

  **Implementation plan**

  - Correct the clean-sheet assignment model so a configuring epoch carries only the stable
    assignment identity, agent, and provider. Bind the provider session only after exact runtime
    readiness, at the runnable transition. Update semantic DTOs, reducer projections, application
    snapshots, and storage v13 in place without compatibility branches, migrations, or a version
    bump. Keep these passive Rust values public.
  - Add an outward transaction-consistent canonical project mutation capability. Every activation
    transition derives a stable per-boundary command identity, validates the exact home, active
    human authority, expected current head, project lifecycle, claimability, and agent cardinality
    inside the commit snapshot, and authors one typed fact or a stable typed rejection. Canonical
    project facts remain the only project authority; saga rows retain coordination checkpoints.
  - Implement the closed activation state machine: observe all desired resources, conditionally
    open a previously closed project, author the configuring assignment, start or exactly resume a
    project-bound runtime, revalidate the explicit launch directory, choose an explicit historical
    thread or the first pending project input, and author the runnable binding. Persist the exact
    acknowledged session and selected thread before later boundaries. On definite failure, end the
    configuring assignment and return to the prior open/closed stable state; on uncertainty,
    require exact lookup and retain the pending human message.
  - Route pending inputs in authoritative home sequence through the harness supervisor's existing
    durable delivery ledger. Derive stable submission/dispatch identities from exact immutable
    input attribution, reconcile pending or uncertain provider acceptance before retry, and author
    `ProjectInputDispatched` only after definite acceptance. Never concatenate backlog input or add
    a competing queue. Re-read canonical assignment and dispatch state before every delivery so
    late output remains attributable but cannot grant current authority.
  - Build deterministic failpoint and restart contracts for every canonical, resource, runtime,
    launch, delivery, and dispatch boundary; stale heads; inactive humans; claim and assignment
    conflicts; start/resume mismatch; first-pending and explicit-thread selection; definite and
    unknown failures; changed stable inputs; accepted-response loss; compensation; bounded repair;
    ordered draining; and complete binding/thread attribution. Run all locked workspace,
    supported-target, architecture, dependency, specification, fuzz, shell, whitespace, and
    unchanged-Go gates.

  **Risks and decisions**

  - A fresh provider session cannot truthfully appear in a canonical configuring fact before the
    provider creates it. The configuring fact therefore records session-free intent; runnable is
    the first canonical state containing the acknowledged session. The durable saga owns the
    interval and compensation stops or releases any runtime that never becomes runnable.
  - Provider acceptance and canonical dispatch cannot share a transaction. The supervisor's
    durable exact-delivery record is the sole bridge: uncertain delivery is reconciled by stable
    identity and digest, and only its accepted state permits the canonical dispatch fact.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement

  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely
  from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any
  other marker. The task and its related subsections should no longer appear in the plan file at
  all. The plan file should not have any sort of "Done" section. Then append a new entry to the
  completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
     preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update
  those. If new future work items were discovered, add them. If the plan file or completed file is
  outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
  other changes.




## 2026-08-28 — Project open and resource mutation workflows

Implemented durable direct open, resource add, forced/unforced remove, and atomic replace workflows
over the existing transaction-consistent canonical project port. Open, add, and replace re-observe
exact resource identity before mutation; the serialized callback independently revalidates home,
human authority, expected head, lifecycle, normalized path identity, assigned-remove force, and the
complete prospective active claim set. Closed desired resources may overlap, while prospective
open claims fail closed against open or closing projects. Every exact canonical mutation remains
checkpointed and strictly encoded for response-loss and restart replay, primary selection retains
its deterministic reducer semantics, and the read-only capability boundary prevents filesystem or
Git mutation. The clean-sheet storage schema remains v13 in place with no migration or version bump.
All locked workspace, strict Clippy, documentation, architecture, dependency, specification,
four-target, fuzz, whitespace, and unchanged-Go gates pass.

### Original plan entry

- **[projects/high] Implement project open and resource mutation workflows** — Implement explicit
  open plus resource add, remove, and atomic replace over the transaction-consistent canonical
  project port. Revalidate exact display/canonical identity and home-qualified claimability before
  mutation; assigned removal requires explicit force, replacement never exposes a partially
  released old claim, and no operation mutates or deletes external resources. Test stale heads,
  inactive humans, claim conflicts, changed observations, assigned force policy, response loss,
  restart repair, and exact primary-resource behavior.

  **Implementation plan**

  - Extend the closed canonical project mutation vocabulary with add, remove, and replace actions.
    Validate immutable home, active-human authority, exact head, lifecycle constraints, resource
    identity, assigned-removal force, and cross-project path claimability inside the serialized
    commit snapshot. Use `hq-resources` pure component-aware claim policy rather than duplicating
    path parsing in the workflow or reducer.
  - Add direct `Open`, `AddResource`, `RemoveResource`, and `ReplaceResource` workflow paths to the
    existing bounded saga manager. Open and new/replacement resources first cross the read-only
    observation boundary with stable identities and exact display/canonical agreement; uncertainty
    remains reconcilable and definite invalid observations leave canonical state unchanged.
  - Checkpoint every exact canonical compare-and-swap before commit and reuse the existing strict
    pending-mutation codec on restart. Resource mutations author exactly one canonical fact; replace
    remains one atomic fact and remove never performs filesystem or Git deletion. Preserve reducer
    primary selection: explicit add-primary, deterministic fallback after removal, and replacement
    of the current primary.
  - Add deterministic contracts for open state and archived rejection, stale heads, inactive
    humans, closed/open claim behavior, local overlap versus cross-project conflict, assigned
    removal with and without force, malformed or changed observations, resource and canonical
    response loss, exact restart replay, atomic replacement, and zero external mutation. Run every
    locked workspace, four-target, architecture, dependency, specification, fuzz, whitespace, and
    unchanged-Go gate.

  **Risks and decisions**

  - Resource inspection is observational and may precede a conflicting canonical commit. The
    transaction callback therefore repeats global claim policy against its exact snapshot; only the
    canonical fact acquires or releases HQ's advisory claim.
  - Closed projects may retain overlapping desired resources because they hold no active claims.
    Opening or mutating an open project must be globally claimable. No resource command changes
    external filesystem or Git state.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement

  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely
  from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any
  other marker. The task and its related subsections should no longer appear in the plan file at
  all. The plan file should not have any sort of "Done" section. Then append a new entry to the
  completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
     preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update
  those. If new future work items were discovered, add them. If the plan file or completed file is
  outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
  other changes.


## 2026-08-28 — Graceful/forced project close and archival workflows

Implemented durable project close, force-close, archive-after-close, and closed unarchive workflows
over the existing saga record and transaction-consistent canonical port. One stable batched,
read-only release assessment applies the shared resource force policy; graceful close enters a
claim-preserving closing state before exact runtime quiescence, while force can revoke HQ authority
after failed or uncertain observations and records that truth in both assignment-end and close
facts. Archive has no implicit force, open archive uses the graceful close path, and closed archive
or unarchive invokes no resource or runtime capability. Exact pending canonical mutations repair
response loss at every boundary without duplicate facts, desired resources and pending inputs are
preserved, and no resource mutation or deletion capability exists in the workflow. The clean-sheet
storage schema remains v13 with no new field, migration, or version bump. All locked workspace,
strict Clippy, documentation, architecture, dependency, four-target, fuzz, whitespace, and frozen
Go gates pass.

### Original plan entry

- **[projects/high] Implement graceful/forced close and archival workflows** — Add durable release
  assessment, graceful runtime quiescence, assignment end, claim-preserving closing, final close,
  archive-after-close, and closed unarchive workflows. Dirty or unknown resources require force;
  graceful close retains claims until runtime quiescence, while force revokes only HQ authority and
  records stopped/still-running/unknown observation without claiming external cessation. Test every
  definite/unknown filesystem, runtime, and canonical boundary, restart repair, pending-input
  preservation, stale commands, competing devices, and no implicit resource deletion.

  **Implementation plan**

  - Extend the read-only project resource capability with one stable batched release assessment and
    apply `hq-resources` pure force policy without interpreting Git-specific evidence in the saga.
    Clean and not-applicable resources may close gracefully; dirty, adapter-unknown, rejected, or
    response-unknown assessment requires explicit force. Assessment never mutates a path or Git.
  - Extend canonical mutations with forced/runtime-attributed assignment end and final close plus
    archive and unarchive actions. Validate immutable home, active-human authority, exact head, and
    lifecycle inside each serialized callback. Entering closing retains the current assignment and
    active claims; only the final closed fact follows assignment end and releases claims.
  - Implement `Close` and `SetArchived` in the bounded saga manager. Graceful close assesses
    release, enters closing, quiesces an assigned runtime by stable identity, ends the exact
    assignment, and closes. Force may continue after rejected or unknown assessment/runtime
    outcomes while recording a truthful typed runtime observation. Archiving an open project uses
    this same graceful path before one archive fact; unarchive remains closed and claim-free.
  - Reuse existing durable effect state and the exact pending-canonical-mutation codec for restart
    repair; do not add a storage field, version, or migration. Preserve accepted pending inputs,
    desired resources, threads, and history throughout. Add deterministic contracts for every
    release/runtime/canonical outcome, response loss, restart point, stale or competing command,
    force gate, archive transition, and zero external deletion, then run every repository gate.

  **Risks and decisions**

  - Runtime quiescence and canonical revocation cannot share a transaction. Graceful close remains
    in claim-preserving closing on definite failure or unresolved response; explicit force may
    revoke HQ authority but records failed/uncertain external observation and never claims the
    process or arbitrary filesystem access stopped.
  - Archive has no force flag. An open archive request performs ordinary graceful close and may
    require the human to force-close separately before retrying archive. Closed archive/unarchive
    never acquires claims or invokes resource/runtime capabilities.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement

  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely
  from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any
  other marker. The task and its related subsections should no longer appear in the plan file at
  all. The plan file should not have any sort of "Done" section. Then append a new entry to the
  completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
     preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update
  those. If new future work items were discovered, add them. If the plan file or completed file is
  outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
  other changes.


## 2026-08-28 — Project handoff, forced takeover, and agent retirement workflows

Implemented durable graceful handoff, forced takeover, and assigned/idle agent retirement over the
existing project saga record. Handoff quiesces and ends the exact old assignment before reusing the
activation path for a distinct available agent and an agent-attributed historical project thread;
definite or unknown graceful stop failure canonically blocks dispatch until an explicit force
records truthful failed/uncertain runtime state. Target activation failure leaves the project safely
open and unassigned. Retirement uses the same quiescence policy, then authors an
installation-private absorbing agent fact only after globally revalidating that the agent is
unassigned, while preserving project lifecycle, claims, desired resources, pending input, threads,
dispatches, and output history. Every canonical boundary has exact response-loss replay coverage,
including blocking, assignment end, activation, dispatch, and retirement. Passive workflow state
remains public Rust data; no accessor layer was introduced. The clean-sheet storage schema remains
v13 with no new field, migration, or version bump. Locked workspace, strict Clippy, architecture,
behavior, causal/protocol, dependency, four-target, fuzz, whitespace, and frozen-Go
build/vet/uncached-test gates pass. The unchanged Go race suite still exposes its pre-existing
three-second mailbox-capability timing failure on macOS; the corresponding uncached non-race suite
passes.

### Original plan entry

- **[projects/high] Implement handoff, forced takeover, and agent retirement workflows** — Quiesce
  and end the old assignment before activating the requested idle agent and exact historical
  thread. A failed graceful handoff becomes canonically blocked; explicit takeover may revoke only
  old HQ authority when runtime cessation is unknown. Retirement ends any assignment before the
  installation-private absorbing agent-retirement mutation while leaving the project open with its
  claims, pending messages, and history. Test compensation, blocked handoff, old/new agent races,
  stale devices, runtime uncertainty, restart recovery, retired-thread rejection, and late output.

  **Implementation plan**

  - Extend the workflow-facing canonical vocabulary with assignment-blocked and agent-retired
    actions. Block and assignment-end remain project-home linear; retirement is authored as an
    installation-private fact with the exact active name claim and local-installation authority.
    The serialized callback revalidates immutable home, active-human command authority, expected
    project head, exact assignment epoch, target agent lifecycle/cardinality, and global absence of
    another assignment before each mutation.
  - Implement handoff as old-runtime quiescence followed by exact assignment end and reuse of the
    existing activation path for the requested agent, provider/session, project-scoped historical
    thread, and launch directory. A definite or unknown graceful stop blocks the old assignment and
    requires a new explicit forced-takeover command. Force records failed/uncertain runtime truth,
    revokes only HQ authority, then activates the new assignment; failed new activation compensates
    to the safe open/unassigned state without resurrecting the old assignment.
  - Implement retirement for an idle local active agent or the exact current project assignment.
    Assigned retirement uses the same graceful/forced quiescence and truthful assignment-end path,
    then authors one absorbing installation-private retirement. The project remains open with its
    claims, accepted pending inputs, desired resources, threads, dispatches, output, and history;
    retired agents and their threads cannot be selected for new runtime authority.
  - Reuse the existing failure, runtime-effect, operation, selected-thread, and pending canonical
    mutation fields for every checkpoint and exact replay. Do not add a storage field, migration,
    or version bump. Add deterministic contracts for stale/inactive requests, same/retired/busy
    target agents, blocked retry policy, stop outcomes, response loss at every canonical boundary,
    activation compensation, retirement absorption, competing commands, and late-output retention,
    then run every repository gate.

  **Risks and decisions**

  - Runtime stop and canonical authority transfer cannot be atomic. Graceful failure therefore
    makes the old assignment canonically blocked and non-runnable; only a later explicit force
    command may end it. A forced end records failed or uncertain observation and never asserts that
    the old external process stopped or lost filesystem access.
  - Handoff is not rollback to the old actor. Once its assignment end commits, later target startup
    failure leaves the project safely open and unassigned. Historical output remains attributable
    and late output cannot regain current authority.
  - Agent retirement is a separate installation-private fact, not a project accessor or storage
    flag. The project head only serializes any preceding assignment end; retirement remains
    absorbing in the agent reducer and does not release project claims.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement

  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely
  from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any
  other marker. The task and its related subsections should no longer appear in the plan file at
  all. The plan file should not have any sort of "Done" section. Then append a new entry to the
  completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
     preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update
  those. If new future work items were discovered, add them. If the plan file or completed file is
  outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
  other changes.


## 2026-08-28 — Durable remote project command routing and local API progress

Implemented complete, strict remote project request, receipt, and terminal outcome projection with
exact control-plane attribution, deterministic conflict handling, and clean reopen/corruption
validation. Non-home routers author only inert requests; home repair workers receipt before
executing the existing durable workflow and publish terminal outcomes only for definite committed
or rejected results. Application-backed fact authoring revalidates active-human and project-home
authority, immutable home, digest/body agreement, exact heads, and request/receipt lineage inside
serialized callbacks. The local API now carries closed typed project actions, requests, outcomes,
runtime observations, and queued/received/terminal/conflicted progress; its passive Rust DTO and
result fields remain public, while reconnect replay preserves byte-identical command frames and
bounded terminal identity history. The unshipped storage and local API schemas were completed in
place: storage remains v13 with no migration or version bump. Locked builds, strict Clippy,
architecture, behavior, causal/protocol, dependency, four-target, fuzz, formatting, whitespace, and
frozen-Go build/vet/uncached-test gates pass. The unchanged Go race suite still exposes its
pre-existing mailbox-capability timing failure on macOS. The otherwise-green Rust workspace suite
also exposes a pre-existing relay test-harness hang: when a local connector fails before TCP
connect, its test server waits indefinitely in `accept()`; all workspace tests pass with that one
unrelated case skipped.

### Original plan entry

- **[projects/high] Implement durable remote project command routing and local API progress** —
  Extend `hq-local-api` with the typed project request/outcome and authoritative checkpoint view.
  Non-home devices author only strict `RemoteProjectCommandRequested` facts; the immutable home
  derives typed receipt parents from one serialized snapshot, executes the same workflow, and
  authors exactly one committed, rejected, or explicitly uncertain result. Validate digest/body
  agreement and expected heads, reject unknown codec versions, and expose queued/received/terminal
  progress without reducer side effects. Test offline routing, competing devices, duplicate and
  changed command identities, stale receipt/result, restart repair, and complete control-plane
  attribution.

  **Implementation plan**

  - Make the reducer-owned remote-command projection retain the complete executable request
    envelope: target home, operation correlation, strict command body, exact request fact, and the
    exact receipt fact once received. Persist those passive fields in the existing clean-sheet v13
    projection table and strict codecs. Change the fresh schema in place; there is no installed
    compatibility boundary, migration, or storage-version bump.
  - Add an application-backed remote-control port that authors request, receipt, and outcome facts
    through one serialized query/commit callback each. Request planning requires exact
    digest/body agreement, active-human authority, observed project head, immutable target home,
    and the prior per-project command frontier. Receipt and outcome planning cite the exact
    request/receipt identities, current or committed canonical head, and project-home authority;
    duplicate stable identities replay while changed bodies or results fail closed.
  - Implement a bounded project-command router around the existing home workflow. A local-home
    command executes that workflow directly. A non-home command authors only its inert request and
    reports `AwaitingHome`. The immutable-home worker scans a deterministic bounded projection,
    authors receipt at its transaction-consistent observed head, reconstructs the exact original
    request with the strict codec, drives the same durable saga, and authors a terminal remote
    outcome only after a definite commit/rejection. Running or reconcilable work remains repairable
    and response loss replays exact fact mutations without duplicate control records.
  - Extend `hq-local-api` v1 in place with closed typed project action/request/outcome DTOs and a
    retry-safe client path. Expose authoritative remote progress as structured queued, received,
    terminal, or conflicted data including expected/received/result heads and typed runtime
    observation. Keep passive DTO fields public; retain opacity only for validated scalar values,
    session-owned tickets, and behavioral capabilities. Add protocol, conversion, server, client,
    router, reducer, store reopen/corruption, and restart-recovery contracts, then run every
    repository gate.

  **Risks and decisions**

  - Control-plane facts never mutate a project. Only the existing home workflow may author
    canonical project facts, and a remote committed result must cite the admitted resulting head.
    A receipt records the head the home actually observed; it does not rewrite the caller's
    expected head or imply success.
  - Relay delivery, workflow execution, and outcome publication cannot be atomic. Exact request,
    receipt, saga, and outcome identities therefore form separate durable boundaries. Unknown
    canonical or runtime truth remains repairable and never becomes a fabricated terminal result.
  - The local API and storage schemas are still unshipped clean-sheet contracts. They may be
    completed in place without compatibility scaffolding, version churn, or accessor layers.

  ## Post-Plan Execution Steps

  Execute these steps in order:

  ### Implement

  Execute the plan above.

  **Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
  make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
  `Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

  ### Verify

  1. Run the project's build/lint command. Fix all warnings.
  2. Run the project's test suite.
  3. If tests fail, fix them before proceeding.
  4. If test coverage for the new work is insufficient, add tests.

  ### Commit

  Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

  ### Update the plan file

  Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely
  from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any
  other marker. The task and its related subsections should no longer appear in the plan file at
  all. The plan file should not have any sort of "Done" section. Then append a new entry to the
  completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

  1. A brief summary, written now, of what was actually implemented.
  2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to
     preserve the original.

  If upcoming plan items need modifications due to a change during this implementation then update
  those. If new future work items were discovered, add them. If the plan file or completed file is
  outside the source repository or is ignored, do not try to stage it; otherwise commit it with the
  other changes.


## 2026-08-28 — Recoverable Git worktree provisioning and concrete project workers

Implemented a recoverable home-local provisioning workflow that reserves one lexically normalized
destination, reconciles or creates an exact Git worktree under bounded execution and per-repository
serialization, identifies it through the read-only resource adapter, and authors one initially open
canonical project. Every external operation and pending canonical mutation has a stable identity
and durable checkpoint; response loss resumes through lookup without duplicate Git or project
creation. Partial branches, stale registrations, files, symlinks, changed identities, and competing
destinations fail closed, while accepted or uncertain orphaned Git state is retained and never
automatically pruned, reset, or deleted. Saga/reservation transaction failpoints prove complete
rollback before commit and exact replay after lost commit responses.

Project creation now represents the absence of a prior head directly with `Option<FactId>`; all
existing-project actions require `Some(head)`, provisioning alone requires `None`, and remote
creation is rejected outside the immutable home. Passive Rust request, configuration, state, and
result data continues to expose public fields without accessor layers. The unshipped v13 schema was
updated in place with a nullable head and reservation lifetime rules, with no migration or storage
version bump. The foreground node now composes a concrete project component over the saga store,
canonical/remote application adapters, shared harness capability, read-only resources, Git adapter,
and post-commit relay wake scheduling, with bounded startup/drain repair and reverse-dependency
shutdown.

Locked workspace tests (excluding the previously documented relay test-harness hang), strict
Clippy, builds/checks, architecture, behavior, causal/protocol, dependency, exact four-target
portable checks, fuzz smoke, formatting, whitespace, and frozen-Go build/vet/uncached-test gates
pass. Dependency policy retains the existing allowed warning for locked yanked `chacha20 0.10.1`.
The unchanged Go race suite reproduces only its pre-existing mailbox-capability timing failure on
macOS; all other race packages pass.

### Original plan entry

- **[projects/high] Implement recoverable Git worktree provisioning and compose project workers** —
  Add a separate bounded mutating Git capability with stable lookup/create operations, short-lived
  repository serialization, destination reservation, exact worktree/branch reconciliation,
  read-only `hq-resources` identification, and one canonical project creation. Resume after every
  reservation, Git, identification, and canonical boundary without duplicate worktree/project;
  never silently delete external state on uncertainty. Compose project workflow, store, harness,
  resources, Git, canonical mutation, wake/recovery, intake, and shutdown ownership in `hq-node`.
  Run bounded startup scans, checkpoint all accepted work before harness/store shutdown, add
  model/failpoint tests for every boundary and reservation conflict, and finish project,
  application/local API, storage, behavior-ledger, acceptance, architecture, and four-target CI
  evidence.

  **Implementation plan**

  - Make an existing project head an explicit optional command precondition: every existing-project
    action requires `Some(head)`, while `ProvisionWorktree` alone requires `None`. Carry that
    distinction through the strict command/remote codecs, projection, local API, saga record, and
    fresh v13 tables in place. Reject remote creation at a non-home installation rather than
    fabricating authority for a project that does not exist. Add no migration, compatibility
    branch, storage-version bump, or accessor layer.
  - Add a closed `GitWorktreePort` with exact lookup and create requests, matching/absent/conflict
    observations, stable operation identity, bounded process time/output, branch validation, and
    per-repository in-process serialization. Reconciliation must prove that the destination is the
    requested worktree of the same common repository and exact branch. A missing destination is
    retryable only when Git has no conflicting branch/worktree registration; partial or ambiguous
    state remains reconcilable and is never pruned, reset, deleted, or force-overwritten.
  - Complete the provisioning state machine over the existing durable destination reservation.
    Checkpoint before Git create, reconcile after every uncertain response, identify the resulting
    destination through a new read-only `ProjectResourcePort` operation, persist that exact
    resource observation, then author one open `ProjectCreated` fact through a serialized
    application callback. Release reservations after definite no-effect rejection or committed
    project ownership; retain them when external Git state may exist without a canonical project.
  - Add a concrete `ProjectNodeComponent` that owns bounded intake and startup repair around the
    saga store, application canonical/remote authoring, harness runtime, read-only path resources,
    mutating Git adapter, and relay wake capability. Startup scans are deterministic and bounded;
    accepted commands are synchronously checkpointed, intake closes before repair drains, and
    shutdown joins owned work before harness and store ownership are released. Prove composition
    and lifecycle ordering with injected ports before wiring the real foreground generation.
  - Add model, fake-port, real temporary-Git, store failpoint/reopen/corruption, local protocol,
    remote rejection, node startup/drain, response-loss, partial-branch/worktree, symlink, changed
    identity, competing destination/repository, and canonical-conflict tests. Update project,
    application, storage, behavior-ledger, acceptance, architecture, and local API specifications,
    then run every locked workspace, four-target, dependency, fuzz, whitespace, and frozen-Go gate.

  **Risks and decisions**

  - Git worktree creation and canonical project creation cannot be atomic. Once Git may have
    created state, HQ retains the destination reservation until exact reconciliation or canonical
    ownership; a definite canonical rejection reports the external worktree without deleting it.
  - Git's repository lock is process-external, while HQ's per-repository lock only prevents its own
    workers from competing. External Git activity may still race any observation, so every accepted
    create is followed by exact lookup and any mismatch fails closed.
  - Project creation has no previous project head. Optionality is represented directly in passive
    command data and validated by action, rather than by a magic fact ID, ignored field, or
    accessor-enforced convention.

## 2026-08-28 — Local client, output, and lifecycle command foundation

Replaced the lifecycle-only argument shim with one installed `hq` command tree for complete help,
build/version inspection, and foreground/status/readiness/stop/restart daemon roles. Plain command
and output data has public fields; validated paths and live clients remain opaque. Human output and
the versioned `hq-cli-output-v1` JSON envelope now cover successes and redacted typed errors with
stable success/failure/usage/unavailable exit statuses and deterministic noninteractive behavior.

Added a bounded synchronous runner around the pure reconnecting local API client plus a strict Unix
transport. The runner serializes response-producing writes to the server's single-flight contract,
caps attempts, reconnect delay, frames, socket operations, and workflow time, correlates semantic
errors and exact request identities, never replays ordinary response loss, and replays retry-safe
mutation/project bytes exactly. Command-only and snapshot-oriented clients select their initial
view explicitly. The concrete `LocalNodeClient` converges coordinator readiness before exposing one
reusable typed request/mutation/project seam, with no direct storage, signer, relay, resource, or
provider access.

Parser/help, deterministic human/JSON, typed exit/redaction, fake transport replay/loss/exhaustion/
incompatibility, real foreground Unix transport, non-TTY, lifecycle restart, and concurrent
autostart tests pass. Strict Clippy, the workspace suite excluding the previously documented relay
test-harness hang, architecture/behavior/causal/protocol/dependency gates, formatting/whitespace,
and frozen-Go build/vet/tests pass. Dependency policy retains the existing locked yanked
`chacha20 0.10.1` warning. No storage schema, migration, or storage-version change was needed.

### Original plan entry

- **[cli/high] Build the local client, output, and lifecycle command foundation** — Replace the
  lifecycle-only argument shim with one coherent installed `hq` command tree, a bounded Unix local
  API runner around `hq-local-api::ReconnectingClient`, concurrent-safe node autostart/readiness,
  stable human and machine-readable output, typed exit diagnostics, and complete help/version/build
  inspection. Keep the explicit foreground daemon role internal to the same executable and give
  later commands one reusable request/mutation/project execution seam. Test parsing, framing,
  response loss, reconnect, incompatible builds, autostart races, non-TTY behavior, redaction, and
  deterministic rendering without direct storage, signer, relay, or provider access.

  **Implementation plan**

  - Define plain public command/output option data and a closed parser for global state-root and
    output-format options, `help`, `version`, and `daemon run/status/readiness/stop/restart`. Keep
    validated runtime paths and live client/session ownership opaque. Render human records and one
    stable versioned JSON envelope from typed results; map failures to documented exit classes with
    no adapter prose.
  - Add a bounded Unix `ClientTransport` and synchronous runner around the existing pure
    `ReconnectingClient`. The runner owns exactly one connection generation, strict framed reads,
    capped retry scheduling, response correlation, ordinary-request loss, byte-identical mutation
    and project replay, subscription refresh, and explicit deadlines supplied by each command.
  - Compose readiness through `NodeClientCoordinator` before local API connection, while foreground
    daemon execution continues to own the node directly. Concurrent absent-node clients may spawn
    candidates but converge on the same ready generation; incompatible protocol/build responses and
    startup diagnostics remain typed and actionable.
  - Add parser/help snapshots, fake-transport state-machine tests, real local-socket lifecycle and
    autostart tests, machine-output fixtures, non-TTY/stdin contracts, redaction checks, and
    architecture rules proving the CLI crosses only node coordination and local API boundaries.

  **Risks and decisions**

  - Retrying ordinary reads can change their observed revision, so only exact mutation and project
    command identities replay automatically. A lost ordinary response is reported and the command
    decides explicitly whether a fresh read is safe.
  - Machine output is a Rust-era contract, not compatibility with Go JSON. It is versioned from its
    first release and contains typed public data or stable error codes, never debug strings.
  - The client runner may block for an explicit workflow deadline, but every socket operation,
    queue, frame, retry delay, and retained identity set remains bounded. Ask/wait semantics that
    intentionally have no routine overall timeout belong to the later messaging slice.

## 2026-08-28 — Installation identity bootstrap and typed local configuration

Added offline `identity init/show/export/import` and typed `config get/set` commands to the one
installed executable. Identity/configuration commands acquire exclusive state ownership and refuse
a live node. Backup export/import requires an absolute path plus explicit `--password-stdin`,
rejects terminal, closed, multiline, malformed, and oversized input, and never accepts or echoes a
password argument. Encrypted import/export remains guarded, atomic, private, redacted, and excludes
history and configuration.

Replaced accessor layers on safe passive `PublicIdentity` and `LocalConfiguration` data with public
fields. Validated relay/provider scalar values and secret/capability owners remain opaque, while the
persistence boundary reconstructs and revalidates caller-mutated configuration before atomic
replacement. Deterministic human and `hq-cli-output-v1` JSON records expose only safe public fields.

Foreground startup now authors exactly one matching `InstallationDeclared` root through the normal
application mutation and store-signing gateway before components or readiness. The mutation uses a
stable command identity and secure auxiliary signing randomness; restart/reopen retains revision
one, fresh snapshot clients work immediately, and an identity/database-root disagreement fails
closed without rewrite or disclosure. No storage schema, migration, legacy reader, or storage
version changed because HQ has no shipped release or standing installations.

Strict node Clippy and all node targets pass. The workspace suite passes with the previously known
relay-package harness hang excluded; architecture, behavior-ledger, causal-spec, protocol-spec, and
dependency gates pass, with only the existing locked yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[cli/high] Bootstrap installation identity and expose typed local configuration** — Add
  `identity init/show/export/import` and typed `config get/set` commands without carrying forward
  routine recursive reset. Ensure the first node startup authors exactly one canonical
  `InstallationDeclared` fact through the application mutation path before publishing readiness, so
  a fresh authoritative snapshot is available. Keep root secrets and backup passwords out of
  arguments, output, RPC, facts, logs, and retained diagnostics; refuse active ownership,
  overwrite, ambiguous stdin, unsafe paths, and noncanonical configuration. Represent safe passive
  public identity/configuration/output data with public fields rather than accessor layers. Test
  first-run/restart bootstrap, encrypted export/import, wrong passwords, pipes/closed stdin,
  deterministic human/JSON output, redaction, configuration replacement, and clean-schema behavior
  with no migration or storage-version bump.

  **Implementation plan**

  - Refine the installed command tree with offline identity and configuration roles. Initialization
    and guarded import acquire exclusive state ownership; show/export/configuration operations
    refuse a live owner rather than reading behind it. Backup passwords come only from an explicit
    bounded input source, are normalized into `BackupPassword`, and are zeroized without entering a
    command argument or diagnostic. Identity output contains only installation ID, signing public
    key, and fingerprint.
  - Make `PublicIdentity` and passive local configuration data idiomatic public-field Rust values.
    Revalidate every caller-constructed configuration at the persistence boundary, retain canonical
    ordering and exact bounds, and keep secret-bearing identity, signer, password, state owner, and
    validated relay/provider scalar values opaque.
  - Before foreground readiness, inspect the authoritative local projection and, only when the
    installation declaration is absent, submit one pure `FactPlan` through the ordinary application
    mutation/store signing path. Bind the fact to the loaded installation and public keys, use no
    direct CLI signer/store access, and make crash/restart convergence return the existing fact
    without duplicate roots or magic compatibility state.
  - Add parser/help/output fixtures, identity/configuration adapter tests, startup bootstrap
    failpoint/reopen tests, real CLI initialization/show/export/import/configuration tests, secret
    redaction and permission adversarial cases, and architecture checks. Update identity, CLI,
    lifecycle, behavior-ledger, and acceptance specifications and run all proportional gates.

  **Risks and decisions**

  - Identity creation and canonical declaration are separate durable boundaries. The identity file
    is authoritative for local root capability; startup reconciles the missing fact under exclusive
    state ownership and does not rewrite or replace an unequal declared installation.
  - Backup passwords must not be accepted as ordinary argv values. A closed or noninteractive input
    fails explicitly rather than prompting forever; later UI integration may supply a separate
    secret-input adapter.
  - All Rust storage contracts remain unshipped. Any clean-schema adjustment is made in place with
    no migration, legacy read path, storage-version bump, or standing-installation compatibility.

## 2026-08-28 — Local human account bootstrap and selection

Added `human create/show/select` to the installed executable. Creator bootstrap reconciles the
reserved human mailbox, a deterministic separately namespaced creator-account identity, and the
frontier-complete local selection through pure application plans and authenticated local-API
mutations. The CLI never receives the root signer or store. Public passive authority inputs and
human presentation records expose fields directly; validated identities and live capabilities
remain opaque.

The clean unshipped local API v1 snapshot now exposes exact installation, mailbox, account, and
selection evidence needed for safe planning, without a protocol or storage version bump. Explicit
snapshot refresh coalesces an in-flight initial view and otherwise loads a fresh complete snapshot.
Local mutation storage now treats a distinct command that authors a byte-identical admitted fact as
a committed semantic no-op at the original revision, retaining its receipt without a false
invalidation.

Two concurrent real CLI creator processes converge on exactly four facts; repeated create remains
at revision four, changed immutable labels and unknown selections add nothing, and restart preserves
the exact human/JSON view. Pure planner, snapshot codec/refresh, mutation receipt, parser/help,
redaction, real-process race/restart, strict Clippy, the workspace suite excluding the known relay
test-harness hang, architecture, behavior-ledger, causal-spec, protocol-spec, and dependency gates pass. Dependency policy retains
only the existing locked yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[cli/high] Bootstrap, inspect, and select a local human account** — Add `human create/show/select`
  over authoritative snapshots and pure application fact plans. Creator bootstrap creates the
  reserved local human mailbox when absent, authors one unique creator account, and selects it
  through exact local-installation and membership authority without CLI signer/store access. Expose
  the exact projection support needed for safe client planning in the unshipped clean local API
  shape, with public passive fields and no protocol/storage version bump. Reconcile partial
  bootstrap and response loss from fresh snapshots; reject ambiguous creator roots, stale selection
  frontiers, inactive membership, changed reuse, and identity/key mismatches. Test restart,
  concurrent create/select, deterministic human/JSON output, redaction, and local-API-only
  architecture.

  **Implementation notes**

  - The creator account ID is SHA-256 domain-derived from the installation ID. It is stable across
    partial bootstrap and response loss but remains a separate raw identity from the installation.
  - Replay-stable creator facts use timestamp zero and the BIP-340 all-zero auxiliary input. Exact
    content still determines the signer nonce; independently racing clients therefore author the
    same canonical fact rather than conflicting roots.
  - Exact-fact convergence required no schema change: canonical evidence was already idempotent;
    the local mutation transaction now retains the second command receipt at the original revision.

## 2026-08-28 — Offline-verifiable human pairing invitations

Added `human invite` and `human join` to the installed executable. The canonical protocol now owns
a strict bounded invitation containing exactly one creator-signed device grant and its complete
transitive signed ancestry. It rejects noncanonical, tampered, missing, duplicate, unsupported, and
extraneous evidence, contains no secret or operational state, and has an explicit no-expiry v1
policy: creator revocation is cancellation.

Pure application planners author frontier-complete grants and exact target-key acceptance from
public-field request records. The clean local API v1 snapshot exposes complete membership frontiers
and creator grant history, while new bounded evidence methods export exact causal closure and
reverify idempotent imports. The clean rebuildable authority schema now retains derived active-grant
attribution in place so repeated export reuses an unrevoked grant; no migration, compatibility
reader, or storage-version change was added.

Invitation files are absolute, bounded, non-symlink regular files created without overwrite behind
a node-owned filesystem adapter. Join verifies the artifact and ordinary authority reduction before
any import, requires the exact local installation and signing key, reconciles the human mailbox,
accepts, and selects. A real two-installation binary test covers wrong target, tamper, duplicate
replay without revision growth, restart, deterministic JSON, and path redaction; protocol, planner,
store, local API, unsafe-path, and existing concurrent revoke/full-regrant reducer tests cover the
remaining boundaries. Full locked workspace tests/build, strict Clippy, architecture, protocol,
causal, behavior-ledger, and dependency gates pass; dependency policy reports only the existing
allowed yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[cli/high] Export and join offline-verifiable human pairing invitations** — Add bounded signed
  invite export and guarded join through pure application plans and the canonical protocol. An
  invitation carries the complete account creator/grant/regrant authority needed for offline target
  verification plus exact target installation/key and bounded relay hints; it contains no root
  secret or local operational state. Join verifies canonical bytes, signatures, target binding,
  lineage, expiry policy if specified, and changed reuse before accepting and selecting membership.
  Test tampering, wrong target/key/account, missing history, duplicate replay, concurrent revoke,
  restart, unsafe paths, and deterministic human/JSON rendering.

## 2026-08-28 — Human device inspection and creator revocation

Added `human devices` and `human revoke INSTALLATION_ID` over the authoritative local snapshot and
ordinary application mutation path. Device presentation includes the permanent creator and every
non-creator device in deterministic order, retaining every exact grant, acceptance, revoke,
membership-frontier fact, signing key, label, and relay hint. Its closed state is creator, pending,
active, revoked, conflicted, or incomplete; multiple current grant lineages and unsupported history
remain explicit instead of selecting a historical winner.

The pure public-field revoke request binds the permanent creator address, account root, exact grant
identity and fact, target installation, and complete current membership frontier. Non-creators,
creator self-revoke, absent history, incomplete/conflicted state, and ambiguous grant attribution
fail closed. Stable mutation replay handles response loss, and repeating a projected revoke is a
semantic no-op. Store fanout explicitly retains the named revoked device as a recipient even after
the atomic membership projection becomes inactive, so the revoke is queued before any later route
block.

The clean unshipped local API v1 shape now includes complete usable acceptance and revoke history in
membership items. Existing normalized authority storage already retained those facts, so storage
remains v13 in place with no field, migration, compatibility branch, or version bump. Planner,
projection codec, conflict presentation, parser/help, fanout, real two-installation creator and
non-creator rejection, replay, and restart tests pass. Full locked workspace tests/build, strict
Clippy, architecture, protocol, causal, behavior-ledger, and dependency gates pass; dependency
policy reports only the existing allowed yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[cli/high] Inspect and revoke human account devices** — Add typed device listing and
  creator-only revoke through authoritative snapshots and pure application plans. Preserve every
  maximal acceptance/revoke, require exact grant attribution, fan revocation out to the named device
  before route blocking, and expose pending/active/revoked/conflicted or incomplete states without a
  chosen historical winner. Test non-creator rejection, stale/incomplete frontiers, concurrent
  acceptance/revoke, regrant ancestry, response loss, restart, fanout, and human/JSON rendering.
## 2026-08-28 — Named-agent catalog and embedded guidance

Added `agent list`, `agent show`, `agent create`, `agent current`, `agent select`, and `agent rename`
through pure application planners and the local API. Catalog presentation preserves active,
retired, incomplete, and conflicted facts without inventing a winner; create reconciles partial
mailbox/name state and can adopt an existing local agent mailbox; session selection and display
rename bind the exact active claim, immutable provider session, project context, and complete prior
register frontier.

Current-session discovery combines supported built-in and custom provider environments into one
fail-closed ambiguity set, and diagnostics do not echo provider session values. Installed `agents`
guidance now covers messaging, retries, synchronization, delivery identity, causal incompleteness,
and human-owned administration. The local API v1 snapshot was enriched in place with the evidence
needed for safe planning and presentation; because HQ has never shipped, storage remains v13 and
the API remains v1 with no migration, compatibility reader, or accessor facade.

Planner, protocol, parser/help, conflict, ambiguity, stale-context, architecture, and real
foreground restart tests pass. The full workspace test suite, formatting check, strict Clippy,
architecture, protocol, causal, behavior-ledger, and dependency gates pass; dependency policy
reports only the existing allowed yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[cli/medium] Implement the named-agent catalog and embedded agent guidance** — Add named-agent
  list/show/create/adopt plus durable session selection and display rename through pure application
  planners and the local API. Discover the current session from supported provider environments and
  ship concise installed guidance for messaging, retry, synchronization, delivery identity, causal
  incompleteness, and human-owned administrative boundaries. Test name and session conflicts,
  provider ambiguity, stale metadata, generated help, and local API-only architecture.

## 2026-08-28 — Safe named-agent retirement workflows

Added `agent retire NAME|AGENT_ID --yes [--force]` through a typed node-owned application
capability and local API request family. The coordinator binds the exact active claim and human
authority, observes the global assignment set, atomically retires an idle agent, or routes an
assigned agent through its owning project's durable saga. Same-project assignment remains eligible
while wrong-home, forked, multiply assigned, stale-claim, changed-identity, and retired state fail
closed.

Assigned retirement reuses project quiescence, block, force, and canonical compare-and-swap
boundaries. Failed or uncertain graceful stops retain the blocked assignment and HQ authority; a
new explicitly forced operation may end it while preserving truthful failed or uncertain runtime
state. Exact operation lookup and byte-identical local request replay make response loss and
coordinator restart repair safe. CLI output verifies the absorbing retirement projection and
reports the owning project and runtime truth when applicable.

Planner, coordinator, force escalation, project-race, stale-head, response-loss, restart-repair,
runtime-uncertainty, protocol, reconnect, parser/help, durable-store lookup, architecture, and real
idle foreground/restart tests pass. Full workspace tests, formatting, strict Clippy, architecture,
protocol, causal, behavior-ledger, and dependency gates pass. The clean unshipped storage remains
v13 and local API remains v1, evolved in place with public passive records and no migration,
compatibility reader, accessor facade, version bump, or dependency change. Dependency policy
reports only the existing allowed yanked `chacha20 0.10.1` warning.

### Original plan entry

- **[cli/high] Implement safe named-agent retirement workflows** — Add named-agent retirement
  through a node-owned workflow that transactionally rejects stale or conflicting state, quiesces
  an assigned runtime through the owning project workflow, and requires explicit force before HQ
  authority can outlive an uncertain stop. Test idle and assigned agents, project races, stale
  heads, response loss, restart repair, runtime uncertainty, and local API-only architecture.

## 2026-08-28 — Interruptible reconnecting TUI client and effect executor

Extended the ordinary reconnecting local API runner with bounded idle polling and observable
generation-scoped connection phases. Delayed reconnect actions retain monotonic deadlines across
short polls instead of being consumed by a poll timeout. The Unix adapter now owns an incremental
frame decoder per connection, so idle timeouts preserve partial frames and normal inactivity is not
misclassified as disconnection. A subscribed `LocalNodeEventClient` shares the ordinary client
composition and broad revision subscription; a real foreground test proves mutation invalidation,
full-snapshot refresh, daemon restart, a new negotiated generation, re-subscription, and recovery.

Added the node-owned `LocalTuiClient` and one bounded `TuiEffectExecutor` worker. Snapshot effects
name their exact semantic section, authoritative local API snapshots map deterministically into
small passive presentation records, and old-section or old-generation results cannot overwrite the
current model. The executor preserves effect identity, releases timers once, coalesces redraws,
reports stable connection failures, and joins on explicit shutdown or drop. Its shutdown drains
bounded results while retrying the stop command, with a regression test that saturates both worker
channels. Architecture checks mechanically prevent the TUI client/executor from importing domain,
application, storage, relay, harness, provider, resource, project, filesystem, or process APIs.

No storage shape, storage version, protocol version, migration, compatibility reader, or accessor
facade was added. Passive snapshot, row, size, failure, and observation records expose public fields;
only invariant-bearing model/effect identities and live capabilities remain opaque.

The locked full workspace check, strict Clippy, all-target/all-feature tests and build, formatting,
architecture, behavior-ledger, causal-spec, protocol-spec, dependency-policy, and protocol-fuzz
gates pass. Dependency policy reports only Ratatui's allowed duplicate `hashbrown`/`syn` versions
and the existing allowed yanked `chacha20 0.10.1` warning. An exact executable-name audit found no
running `hq` daemon after the tests.

### Original plan entry

- **[tui/high] Build the interruptible reconnecting TUI client and effect executor** — Extend the
  ordinary local API client driver with bounded idle polling, preserve partial frames across poll
  timeouts, and compose subscription/reconnect observations into one TUI effect executor. Add
  section-bound snapshot effects, deterministic local-API-to-presentation mapping, stale-result and
  invalidation tests, bounded timer/redraw coalescing, and joined worker shutdown. Complete this
  work when the executor remains responsive during reconnect and performs no direct domain/storage
  access.

## 2026-08-28 — Crossterm terminal shell and installed TUI routes

Added a node-owned Crossterm/Ratatui terminal capability and shell around the pure UI reducer and
bounded effect executor. Backend events normalize into the closed UI vocabulary, redraws always
borrow the latest complete model, and the shell reaches authoritative state only through the
subscribed ordinary local client. An ordered activation state and an armed RAII guard restore the
cursor, mouse capture, alternate screen, and raw mode exactly once after normal quit, Ctrl-C,
terminal failure, partial activation, client-worker panic, or outer panic unwinding.

The single installed executable now accepts `hq tui`; bare `hq` selects the TUI only when both
stdin and stdout are terminals and retains noninteractive `list` behavior otherwise. Machine output
is rejected for the interactive role, and nonterminal explicit invocation returns a stable usage
diagnostic without emitting terminal escapes. Linux/macOS pseudoterminal coverage proves explicit
and bare routing, visible rendering, alternate-screen exit, and exact termios restoration. The PTY
test also caught and removed a redundant Crossterm cursor-position query during activation.

Passive terminal observations expose their data directly; only the live terminal capability,
ordered activation invariant, model, and executor retain private state. No storage shape, storage
version, protocol version, migration, compatibility reader, or accessor facade was added. The full
locked workspace check, strict Clippy, all-target/all-feature tests and build, architecture,
behavior-ledger, causal-spec, protocol-spec, dependency-policy, and protocol-fuzz gates pass.
Dependency policy reports only Ratatui's allowed duplicate `hashbrown`/`syn` versions and the
existing allowed yanked `chacha20 0.10.1` warning.

The full parallel Unix CLI suite exposed one autostarted daemon retaining a caller-owned descriptor
on macOS; stopping that exact temporary-state owner let the existing suite complete. The distinct
descriptor-inheritance defect is now the first remaining PLAN package rather than being hidden or
folded into terminal behavior. An executable-path process audit is clean after that explicit
cleanup and all TUI test guards.

### Original plan entry

- **[tui/high] Compose the Crossterm terminal shell and installed TUI routes** — Add terminal input
  mapping, the redraw/event loop, installed `hq tui` and bare-terminal roles, and RAII terminal
  restoration. Add normal, error, cancellation, and panic restoration tests plus installed-binary
  pseudo-terminal coverage. Complete this work when the shell is usable, restores every terminal
  mode on every exit path, and reaches state only through the TUI effect executor.

## 2026-08-28 — Autostarted daemon descriptor isolation

The installed foreground `daemon run` role now enumerates its process descriptor directory before
runtime or worker startup and closes every inherited descriptor above the three standard streams.
Linux uses `/proc/self/fd`; macOS uses `/dev/fd`. The implementation uses safe `nix`
closure only, preserving the workspace-wide `unsafe_code = forbid` policy, and changes no caller
descriptor flags or ownership.

A subprocess regression opens a deliberately inheritable pipe, starts a live foreground node,
drops the caller's writer, and proves the read side reaches EOF while the daemon remains ready.
The formerly hanging concurrent-readiness test now completes normally because its autostarted owner
cannot retain the test harness capture channel. The complete Unix CLI integration file runs without
manual intervention, and an executable-path process audit finds no debug or release `hq` owner
afterward. The parallel serialization self-test also uses a suite-scale eventual-acquisition bound
instead of assuming unfair mutex wake order.

Formatting, locked full-workspace check, strict Clippy, all-target/all-feature tests and build, and
the architecture gate pass. No storage or protocol shape/version, migration, compatibility reader,
or accessor facade changed.

### Original plan entry

- **[node/high] Prevent autostarted daemons from inheriting caller descriptors** — Close every
  unrelated inherited descriptor in the spawned daemon before `exec` while preserving only the
  explicit null standard streams. Add Linux/macOS process tests proving an autostart caller whose
  output is captured reaches EOF, aborted callers leave no descriptor references that obstruct
  cleanup, and exact executable-path audits find no orphaned test daemons. Complete this work when
  the Unix CLI suite finishes without manual daemon intervention and every spawned owner is
  independently stoppable and reapable.

## 2026-08-28 — Authoritative TUI mailbox conversations and activity

The pure TUI now presents open, sent, and archived conversation summaries from store-derived
projection counts, then loads mixed message/activity history through the ordinary bounded local-API
conversation query. Opaque cursors and exact effect identities reach the single client worker;
stale page results cannot replace a newer selection or post-invalidation repair. The model retains
summary selection, open conversation, reducer fact anchor, focus, and typed-detail disclosure
across applicable reload, reconnect, invalidation, and resize paths.

Conversation pages remain in reducer order and the TUI performs no activity coalescing or domain
ordering. Activity is a distinct non-actionable entry family. Its local-protocol status is now a
closed enum that preserves bounded failure reasons rather than a behaviorally interpreted string.
Messages expose typed routing, semantic correlation, causal frontier, receipt, thread, state, and
project disclosure. Passive snapshots, rows, pages, entries, and disclosure records have public
fields; only the invariant-bearing model and effect identity retain private state.

The responsive renderer adds an anchored conversation pane, paging and detail controls, explicit
non-actionable activity labels, incomplete/truncated diagnostic rows, and stable actionable page
failures. Model, renderer, mapping, protocol, executor, and key-normalization tests cover
open/sent/archived filtering, reducer order, pagination, stale completion, reconnect repair,
invalidation, resize, typed activity failure, and terminal sanitization.

Formatting, locked full-workspace check, strict Clippy, all-target/all-feature tests and build,
architecture, behavior-ledger, causal-spec, protocol-spec, dependency-policy, and 512-run protocol
fuzz gates pass. No storage schema or storage version changed. Because HQ is unshipped, the derived
snapshot fields and typed activity status update the current local protocol v1 directly without a
migration, compatibility shape, or protocol-version bump. Dependency policy reports only the
existing allowed Ratatui duplicate and yanked-transitive warnings. An executable-path process audit
finds no remaining debug or release `hq` daemon.

### Original plan entry

- **[tui/high] Present authoritative mailbox conversations and activity** — Extend the ordinary
  local-API TUI client and pure model with open, sent, and archived filters; reducer-ordered mixed
  conversation/activity pages; typed technical disclosure; stable conversation selection; and
  logical scroll anchors. Preserve those presentation choices across authoritative reload,
  invalidation, reconnect, and resize, consume reducer activity coalescing without reimplementing
  it, and show stale or conflicted data as typed actionable state. Complete this work when mailbox
  browsing is fully snapshot-driven and pure model/render tests prove canonical presentation and
  state preservation.

## 2026-08-29 — Assignment-aware agent status

Agent rows now describe what a person can act on: active agents without project work are
`unassigned`; assigned agents name their project and distinguish `setting up` from `ready`; blocked
or conflicting states say `needs attention`; and retired agents say `retired`. The mapping is based
on typed lifecycle and current-assignment evidence rather than display strings or inferred runtime
presence.

Agent details retain the exact project, assignment, provider, session, and block evidence behind
the plain-language status. Mapper, pure-model, and responsive render tests cover unassigned,
assigned, blocked, conflicted, and retired states. Formatting, architecture checks, strict Clippy,
the locked full-workspace test suite, and the all-target/all-feature workspace build pass.

### Original plan entry

### Present assignment-aware agent status

Replace the generic `waiting` presentation with typed user-facing agent status derived from the
agent lifecycle and current project assignments, without parsing display strings or inventing
runtime presence.

- Present a newly created active agent with no project assignment as `unassigned`.
- Present an assigned agent with the project name and distinguish configuring, available/runnable,
  and blocked assignments in plain language supported by typed snapshot fields.
- Reserve `needs attention` for conflicts or blocked state and `retired` for retirement. Do not claim
  `running` or `idle` without typed runtime-presence evidence.
- Keep stable IDs, provider/session selection, and assignment evidence available in details rather
  than using them as the primary row status.
- Add mapper, pure-model, and render tests for unassigned, assigned, blocked, conflicted, and retired
  agents at narrow and wide terminal sizes. Update `docs/rust/tui.md` with the status vocabulary.

## 2026-08-29 — Persistent contextual TUI help and focused footers

Every ordinary TUI section now opens contextual help with `?`, including empty sections and
sections with a selected item. Context help explains the section, the selected item's
plain-language state, and the applicable key reference; `t` switches to stable identities,
authoritative revision, connection state, and recovery evidence. Help owns background input while
open, survives resize and authoritative refresh, and closes with `?` or Escape.

Ordinary footers now show only the most relevant available actions and consistently expose
`? help`. Inapplicable message actions produce dismissible prerequisite guidance instead of a
persistent failure, including a specific explanation that activity updates are not actionable.
Pure-model and responsive render coverage exercises all five sections with and without selections
at narrow and wide sizes. Formatting, architecture checks, strict Clippy, the locked full-workspace
test suite, and the all-target/all-feature workspace build pass.

### Original plan entry

### Add persistent contextual TUI help and focused footers

Make help available before and after the TUI contains data, while keeping the ordinary footer
focused on immediate actions.

- Add an always-available `?` contextual help overlay. It must explain the current section, list all
  available actions in plain language, describe the selected item's user-facing state, and offer a
  separate technical-details view for identities and recovery evidence.
- Simplify footers to the few actions relevant in the current context and use labels such as
  `c create`, `n new`, `? help`, and `Enter open`. Keep the complete key reference in contextual
  help rather than forcing every shortcut into the status bar.
- Treat guidance caused by an inapplicable shortcut as transient help, not a persistent operation
  failure. Explain the prerequisite and dismiss the hint on the next meaningful input.
- Add pure-model tests and render snapshots for help opened from every section, with and without a
  selected item, at narrow and wide terminal sizes. Document the help contract in `docs/rust/tui.md`.

## 2026-08-29 — Empty states with exact next actions

Inbox, Sent, Archived, Agents, and Projects now replace the generic `No items` presentation with a
plain-language explanation of what belongs there and one or two actions that work from the current
screen. Project guidance leads with durable work and folder/resource ownership, while isolated Git
worktrees remain a disclosed secondary option instead of the empty screen's product definition.

An empty direct-message recipient chooser now explains that nobody is reachable, points to agent
creation as the available path, and leaves room for people in the user's future HQ network. It
hides selection and compose controls until a typed recipient exists, while pure-model coverage
proves navigation and submission remain inert and Escape is safe. Narrow/wide render coverage spans
all five empty sections and the chooser. Formatting, architecture checks, strict Clippy, the locked
full-workspace test suite, and the all-target/all-feature workspace build pass.

### Original plan entry

### Explain empty states with exact next actions

Give every empty section and empty recipient chooser a plain-language purpose and one or two
actions that are possible from the current screen.

- Distinguish an empty Inbox, Sent, and Archived section; explain what each normally contains and
  point to the applicable direct-message, personal-note, or `New...` action without implying that
  project work is the only collaboration path.
- Explain that an empty Agents section contains no named workers yet and offer agent creation. For
  an empty Projects section, explain project/resource ownership and offer project creation without
  positioning HQ as a worktree manager.
- When no direct-message target is available, explain that no reachable recipient exists, offer the
  applicable agent-creation path now, and leave the copy compatible with future human recipients.
  Do not render selection or submission controls that cannot work.
- Keep the contextual-help and focused-footer actions consistent with each empty state. Add pure
  model tests and narrow/wide render snapshots for every section and the empty recipient chooser.

## 2026-08-29 — Typed human-account recovery

The node mapper now distinguishes every local condition that can prevent safe human-account use:
no selection, unresolved candidates, duplicate selection records, a selected account without
local authority, pending or revoked membership, and conflicting membership or acceptance evidence.
The pure TUI receives closed typed evidence rather than parsing diagnostic prose.

Ordinary screens explain the exact condition and show only applicable create, join, select, sync,
repair, or retry actions. Contextual technical help preserves stable recovery codes, candidate and
selected account IDs, selection and membership frontiers, membership status, and active acceptance
evidence; wide layouts show full IDs while narrow layouts remain bounded. The former aggregate
ambiguity message and its instruction to inspect unrelated IDs are gone.

Mapper, model, responsive render, and installed pseudoterminal tests cover every condition. The
typed recovery vocabulary is documented in `docs/rust/tui.md`. Formatting, architecture checks,
strict Clippy, the locked full-workspace test suite, and the all-target/all-feature workspace build
pass.

### Original plan entry

### Explain unavailable and conflicted human accounts from typed evidence

Replace the aggregate unavailable/ambiguous human-account presentation with exact typed conditions
and applicable recovery actions derived by the node mapper.

- Distinguish no local account selection, several local selection candidates, several local
  selection records, a selected account with no local creator/device authority, and conflicting or
  inactive local membership evidence without parsing diagnostic prose.
- Explain each condition in ordinary language and offer only the applicable create, join, select,
  sync, repair, or retry action. Do not make the user inspect unrelated IDs to discover the problem.
- Preserve candidate account IDs, selection frontier, membership/authority evidence, and stable
  recovery codes in contextual technical details.
- Add mapper, pure-model, and narrow/wide render coverage for every unavailable and conflicted
  condition. Update `docs/rust/tui.md` with the typed recovery vocabulary.

## 2026-08-29 — TUI vocabulary and progressive disclosure

Ordinary TUI surfaces now describe user intentions and visible outcomes instead of HQ internals.
Headers report the device connection in plain language; activity is an information-only update;
projects lead with status, folders and resources, and the assigned agent; agent workflows use saved
conversations and agent services; mailbox dialogs explain who receives a message; and project
forms, confirmations, completions, and failures say what HQ needs, will change, or kept on disk.

Stable IDs, revisions, raw connection and runtime states, request identities, provider/session and
thread evidence, causal details, failure codes, and recovery actions remain available behind
explicit technical labels. Ordinary failure footers now explain the recovery action without making
the user interpret a stable code. Compact layouts retain required fields, warnings, actions, and
recovery guidance before secondary evidence.

Responsive render contracts cover the revised ordinary and technical states at narrow and wide
sizes, and the installed pseudo-terminal workflows synchronize on stable structural cues rather
than presentation phrases. The final vocabulary and progressive-disclosure contract is recorded in
`docs/rust/tui.md`. Formatting, architecture verification, qualification-evidence validation,
strict workspace Clippy, the locked full-workspace test suite, the workspace build, and installed
terminal scenarios pass.

### Original plan entry

### Complete the TUI vocabulary and progressive-disclosure audit

Audit every ordinary screen, dialog, footer, hint, and exceptional outcome from the perspective of
a person seeing HQ for the first time.

- Replace unexplained reducer, transport, authority, reconciliation, provider-session, assignment,
  and thread terminology where it is not essential to the user's decision.
- Ensure every screen explains what it represents, why input is needed, what the user can do next,
  and what will happen afterward.
- Keep stable identities, causal evidence, raw state codes, and recovery diagnostics available in
  technical details rather than deleting them.
- Add or update render snapshots and pure-model tests for every revised state at narrow and wide
  terminal sizes. Update `docs/rust/tui.md` with the final vocabulary and progressive-disclosure
  contract.

## 2026-08-30 — Configurable semantic TUI themes

HQ now resolves one immutable, complete semantic theme before terminal activation and passes it
through the borrowed renderer without putting visual policy or filesystem access in the UI model.
Thirty independently configurable roles cover full-screen and modal surfaces, ordinary/muted/
technical text, inputs and cursor, selections, borders, footers, connection and row states, and
success/warning/error feedback. The renderer paints every frame cell and every cleared overlay;
source and style-buffer tests prevent concrete color policy or unthemed surfaces from returning.

Unsigned local configuration and the generated CLI grammar now support config get, config themes,
and config set theme NAME_OR_ABSOLUTE_PATH|none while preserving byte-identical legacy version-1
files when unset. Startup honors NO_COLOR only without an explicit choice. Discovery is bounded and
fail-closed for unsafe files, symlinks, traversal, duplicate/reserved names, malformed TOML or
Base16 YAML, unknown roles and fields, bad colors/modifiers, cycles, and excessive inheritance.
Invalid selected themes produce a pre-terminal diagnostic naming the failed subject.

Bundled choices include terminal, no-color, and six pinned, attributed Gruvbox dark/light
hard/medium/soft Base16 palettes. Native TOML supports palettes, inheritance, partial semantic
overrides, ANSI/indexed/RGB colors, underline colors, and Ratatui modifiers. User and architecture
documentation contains the complete role reference, copyable examples, discovery and precedence,
Base16 mapping, accessibility behavior, and terminal color limitations. Formatting, architecture,
Go-independence, dependency-policy, qualification, strict workspace Clippy, locked workspace
check/test/build, and installed PTY coverage all pass.

### Original plan entry

### Add a complete, configurable TUI theme layer

Replace hard-coded Ratatui colors with a startup-loaded theme system. Preserve today's appearance
as the default `terminal` theme, while allowing users to select bundled themes or define every
visual role—including the full-screen background and ordinary text foreground/background—without
recompiling HQ. Theme changes take effect the next time the TUI starts; runtime switching and file
watching are explicitly out of scope.

Research found no universal Ratatui or TUI theme schema. Ratatui provides [color and style
primitives](https://docs.rs/ratatui/latest/ratatui/style/enum.Color.html), while mature TUIs such as
[Helix](https://docs.helix-editor.com/themes.html) and
[Zellij](https://zellij.dev/documentation/themes.html) define application-specific semantic roles,
named built-ins, palettes, inheritance, and user theme directories. Follow that model for complete
HQ customization. Use [Tinted Theming/Base16](https://github.com/tinted-theming/home) as a palette
interchange format and source of reusable choices, not as HQ's complete element schema: its palette
can map background, normal text, muted text, selection, accent, success, warning, and error colors,
but it cannot name all of HQ's dialogs, focus states, and status treatments.

#### Theme model and rendering boundary

- Add a presentation-only theme module in `crates/hq-tui`, with tests written alongside the model
  before replacing renderer styles. Keep `hq-tui` pure: it may define and validate passive
  theme/style values, but it must not read files, configuration, environment variables, or terminal
  capabilities.
- Represent a resolved theme as a complete immutable catalog of semantic roles rather than a bag of
  concrete color names. Inventory every distinct styled element in `render.rs` and cover at least
  the root surface and normal text; muted and technical text; headings and accents; focused and
  unfocused selections and borders; modal surface, border, and title; header badge; input and
  cursor; ordinary and status footers; connection and row states; and success, warning, error, and
  attention text. Each style role must support foreground, background, underline color, and the
  Ratatui modifiers that the terminal backend can express. Inheritance and fallbacks belong only in
  definition resolution; the renderer receives a complete theme.
- Change the borrowed render boundary to accept `&UiTheme`, without putting visual policy in
  `UiModel`. Replace every direct `Color::*` decision in `render.rs` with a semantic role, and add a
  source-level guard that prevents new concrete colors from creeping back into the renderer.
- Paint the root style across every cell so unstyled text and blank space receive the configured
  normal foreground and background. Ratatui's `Clear` restores cells rather than a themed modal
  surface, so explicitly repaint every overlay area after clearing it. Verify nested dialogs and
  help overlays as well as ordinary screens.
- Keep state and focus understandable without color. Preserve text labels, selection markers,
  borders, and modifiers so `no-color` and limited terminals do not make interactions ambiguous.

#### Configuration, discovery, and native theme files

- Extend the existing unsigned `LocalConfiguration` and `hq config` grammar with an optional theme
  selection: `hq config set theme NAME_OR_ABSOLUTE_PATH`, with `none` restoring automatic/default
  selection. Include the setting in human and JSON `hq config get` output. Preserve byte-for-byte
  acceptance of existing canonical version-1 files: an unset theme must deserialize by default and
  remain omitted when re-encoded. Retain bounded input, exact canonical validation, atomic private
  replacement, and symlink rejection.
- Add `hq config themes`, or an equally discoverable typed command, to list bundled and valid user
  themes, mark the active selection, and report invalid definitions without making users guess
  identifiers. Built-in names are reserved and lookup must reject ambiguous duplicate user names.
- Resolve the selected theme once in `hq-node` before activating raw mode or the alternate screen,
  then pass the immutable result through `tui_shell` to rendering. F5, reconnect, and authoritative
  snapshot refreshes must not reload or change it. A missing or invalid selected theme must produce
  an actionable pre-terminal diagnostic that names the file and offending field; never silently
  switch to another palette.
- Discover named user themes under `$XDG_CONFIG_HOME/hq/themes`, falling back to
  `~/.config/hq/themes`, while continuing to allow an explicitly configured absolute file. Keep
  filesystem resolution out of `hq-tui`. Bound file size and inheritance depth; reject symlinks,
  traversal or ambiguous names, cycles, unknown fields, invalid colors or modifiers, and unresolved
  palette references.
- Define an HQ-native TOML format inspired by Helix: optional `inherits`, a named `[palette]`, and
  partial semantic style entries. Foreground, background, and underline values accept `reset`,
  named ANSI colors, `ansi:N`, `#RRGGBB`, or palette references; modifiers are explicit bounded
  lists. Document every role and ship a complete example that overrides ordinary text and the
  screen, modal, and selection backgrounds. A partial theme resolves through its parent and the
  selected root definition, never through whatever style happens to be underneath a widget.

#### Ecosystem compatibility and bundled choices

- Support local, offline import of the current Tinted Theming Base16 YAML scheme format. Map
  `base00` to background, `base05` to normal text, `base03` to muted text, `base02` to selection,
  `base08` to error, `base0A` to warning, `base0B` to success, and `base0D` to accent, then derive a
  complete HQ theme and allow native semantic overrides. Preserve scheme name and author in theme
  listings and diagnostics. Do not fetch themes during startup, claim Base16 defines HQ's semantic
  roles, or couple the renderer to the import format.
- Ship `terminal`, `no-color`, and Gruvbox dark/light hard/medium/soft presets. Pin their source to
  the MIT-licensed [Tinted schemes](https://github.com/tinted-theming/schemes), retain attribution,
  and structure the import/generation path so later presets do not require hand-copying every HQ
  role. Do not vendor the entire upstream catalog in this task.
- With no configured choice, use `terminal` for compatibility. If `NO_COLOR` is nonempty and the
  user has not explicitly selected a theme, use `no-color`; an explicit configuration choice wins,
  following the [NO_COLOR convention](https://github.com/jcs/no_color). `no-color` may keep
  non-color modifiers such as bold and reverse for focus.
- Accept both terminal-native ANSI/indexed colors and RGB, but do not promise silent or exact RGB
  conversion on terminals without truecolor support. Document the limitation and the
  `terminal`/ANSI alternatives; theme resolution must be deterministic and must not mutate during
  drawing.

#### Tests, documentation, and completion

- Add focused tests first for color and style parsing, palette references, inheritance and override
  precedence, cycle and depth rejection, unknown fields, malformed and oversized files, Base16
  dark/light mapping, and deterministic bundled-theme generation.
- Extend identity/configuration and CLI tests for legacy config acceptance, canonical persistence,
  selection/list/get behavior, missing files, unsafe or ambiguous paths, invalid user themes, and
  actionable pre-terminal errors. Test `NO_COLOR` precedence without depending on ambient process
  environment.
- Extend `crates/hq-tui/tests/render_snapshots.rs` with style-aware buffer assertions proving that a
  custom ordinary foreground/background covers the entire screen, modal surfaces retain their
  background after `Clear`, independently overridden focus and status roles reach the intended
  cells, `no-color` retains non-color focus cues, and the default theme preserves existing text and
  layout snapshots.
- Update `docs/rust/tui.md`, `docs/rust/cli.md`, and user-facing configuration documentation with
  startup semantics, search paths, theme discovery, the complete native role reference, Base16
  mapping, bundled names, accessibility behavior, terminal color limitations, attribution, and
  copyable examples.
- Finish with formatting, architecture verification, dependency-policy audit for any new parser
  crates, strict workspace Clippy, and the complete locked workspace test and build suite.

## 2026-08-30 — Lighter project-folder fields

One-line dialog fields no longer insert a literal pipe character into the rendered value. The
shared renderer now applies the configurable cursor style to the actual character under the caret,
or to one trailing blank cell at the end, while preserving Unicode-safe byte boundaries. Focus
remains visible through the field marker and theme styles without the extra glyph.

Required and optional hints now appear only while a field is empty and disappear as soon as it has
content, including the path, generated project name, brief, agent name, and other shared one-line
forms. Render coverage proves the cursor cell is themed but blank, filled required and optional
fields omit their hints, empty fields retain guidance, and no field-adjacent pipe returns. The TUI
form contract was updated, and formatting, architecture, Go-independence, qualification, strict
workspace Clippy, locked workspace check/test/build, pure TUI, and installed PTY suites all pass.

### Original plan entry

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

## 2026-08-30 — Obvious fields and reliable guided project startup

One-line dialog controls now have a full-width subdued surface, a distinct focused surface, a
theme-composed caret, a gap after the label, and right-aligned empty required/optional hints.
Display-cell-aware clipping keeps Unicode values and narrow dialogs safe. The semantic theme
contract covers terminal, no-color, Base16-derived, and inherited native themes.

The guided New flow now skips the provider confirmation when exactly one service is available,
retains explicit choice and handoff review where they matter, submits a new project's first
instruction before activation, correlates its accepted message to the exact authoritative thread,
and dispatches it once after the assignment becomes runnable. Threadless activation is rejected
before assignment or runtime effects. Snapshot/local API/CLI/TUI boundaries expose pending input
thread identity, and canonical automated project transitions now carry the account and typed
authorities required by the reducer and protocol.

An installed pseudoterminal regression exercises the complete post-bootstrap project/agent/
instruction path with a deterministic Codex adapter process. Formatting, architecture and protocol
specification checks, strict workspace Clippy, locked all-target/all-feature tests and builds, and
the full release qualification suite pass.

### Original plan entry

### Make editable fields obvious and make fresh project work reliable

Make one-line dialog inputs read as actual controls, and restore the guided `n` workflow so a new
user can create a project and agent, provide the first instruction, and start work without a
redundant single-provider confirmation or an activation failure.

The reported fresh-state failure is:

- Dialog: `Project work` / `Set up project work`
- Project: `2a04adc36452`
- Request: `89d3719a2caf`
- Runtime: `succeeded`
- Reason: `conflict/project_activation_thread_missing`

#### Editable field surfaces

- Add tests first around the shared one-line field renderer in `crates/hq-tui/src/render.rs`, then
  make it width-aware. Keep the label and colon outside the input surface, leave visible horizontal
  space after the colon, and pad the input surface through the remaining inner width of the dialog.
- Give the complete padded input surface an obvious subdued background while unfocused and a
  distinct focused treatment when selected. Keep the insertion caret visible by composing its
  semantic cursor style over the focused field style; do not insert a cursor glyph into the value.
- While an input is empty, right-align `(required)` or `(optional)` inside the input surface. Remove
  the hint as soon as content exists. The hint, cursor, value, and background must not overlap or
  spill outside the dialog at supported narrow and wide sizes.
- Apply the shared treatment to every one-line editable dialog field that uses the common renderer:
  project and agent searches, project creation/resource/activation fields, agent creation, and
  saved conversation naming. Audit other dialog inputs and reuse the component where they have the
  same one-line editing contract; do not force choice rows or the multiline message editor into it.
- Preserve Unicode-safe caret behavior and use terminal display width rather than UTF-8 byte length
  when padding, clipping, or keeping the caret visible.
- Add explicit semantic focused and unfocused field-surface theme roles, or an equivalently clear
  additive theme contract. Give `terminal`, `no-color`, and Base16-derived themes accessible
  defaults, keep focus discernible without color, and update native-theme parsing/coverage and
  `docs/tui-themes.md`.

#### Single-provider and first-instruction flow

- In `crates/hq-tui/src/model.rs`, show `Start project work` only when the user has more than one
  valid agent-service choice. Automatically use the sole available service. Preserve an actionable
  unavailable state when there is no usable service, automatically retain an exact historical
  provider/thread binding when resuming, and keep explicit review for a real assignment move or
  handoff.
- Repair the new-project ordering instead of weakening thread identity. When the selected
  project/agent has no compatible historical thread, collect and durably submit the first project
  instruction before activation, wait until the authoritative snapshot exposes its accepted
  thread, then activate using that exact thread, wait for the runnable assignment, and dispatch/open
  the resulting project conversation exactly once.
- Retain the draft and exact project, agent, provider, and thread correlation across refresh,
  reconnect, rejection, and reconcilable outcomes. A stale completion or retry must not silently
  activate a different target or duplicate the initial instruction.
- Strengthen `crates/hq-projects/src/workflow.rs` so activation verifies that an explicit historical
  thread exists, or that a first pending input supplies a thread, before configuring an assignment
  or starting the runtime. A threadless activation must fail without leaving a succeeded/orphaned
  runtime or requiring late compensation.
- Keep the advanced project activation form and CLI contract intact: explicit historical resumes
  still require the exact thread/session pair, and ordinary pending-input activation still selects
  and dispatches the first accepted input.

#### Verification and documentation

- Extend `crates/hq-tui/tests/render_snapshots.rs` with style-aware cell assertions proving field
  padding reaches the dialog edge, empty hints sit at the right edge inside the field, filled fields
  omit hints, focused and unfocused surfaces differ, the caret remains visible, and narrow/Unicode
  cases do not wrap or corrupt values.
- Extend `crates/hq-tui/tests/model.rs` for zero, one, and multiple available services; exact
  historical resume; handoff review; refresh/reconnect retention; and the no-history path from first
  instruction through activation and exactly-once dispatch.
- Extend `crates/hq-projects/tests/activation_dispatch.rs` to prove missing thread prerequisites are
  rejected before canonical assignment or runtime effects, while a pending first input selects its
  exact thread and completes normally.
- Add an installed or cross-layer regression in `crates/hq-node/tests/unix_tui_terminal.rs` that
  follows the post-bootstrap-from-nothing `n` path through project creation, agent creation, one
  automatically selected provider, initial instruction, runnable assignment, and conversation
  opening. It must not render the redundant `Start project work` screen, report
  `project_activation_thread_missing`, orphan a runtime, or send the instruction more than once.
- Update `docs/rust/tui.md` to match the actual provider-skipping and first-instruction ordering,
  plus the full-width focused/unfocused field contract. Finish with formatting, architecture and
  qualification checks, strict Clippy, and the locked focused and workspace test suites.

## 2026-08-30 — Runtime delivery project provenance

Project dispatch now carries its stable dispatch identity into a typed harness provenance record,
and the durable delivery ledger retains the project, dispatch, assignment, thread, and positive
input sequence before provider I/O. Exact retries remain idempotent while any changed provenance
collides.

The SQLite store now upgrades known v1 databases transactionally to schema v2. Existing delivery
rows survive as explicitly unattributed, complete new attribution round-trips across repair and
restart, and partial attribution fails closed. Node/store conversion tests, activation dispatch
tests, migration/reopen tests, strict workspace Clippy, and the locked full workspace suite pass.

### Original plan entry

### Retain exact project provenance in the runtime delivery ledger

Stop discarding project identity at the boundary between project dispatch and the managed runtime.
Every newly queued project delivery must durably retain its project, accepted input, dispatch,
assignment binding, selected project thread, and input sequence. Existing databases must migrate
in place, and legacy delivery rows whose attribution was never stored must remain explicitly
unattributed rather than being inferred from names, current assignment, or provider/session
coincidence. This task deliberately stops at the durable provenance seam; the next task will use
that evidence to author canonical project output and activity.

#### Implementation plan

1. Write the failing boundary and storage tests first.
   - In `crates/hq-node/src/harness_component.rs`, extend the retained-delivery identity test so
     changing any project, dispatch, assignment, thread, input, or sequence field causes an exact
     identity mismatch, while a byte-for-byte retry remains idempotent.
   - In `crates/hq-store/tests/harness_state_contract.rs`, require complete attributed delivery
     round-trip behavior across repair and reopen, collision on changed provenance, preservation of
     an explicitly unattributed legacy row, and successful opening/migration of a real v1 fixture.
   - In `crates/hq-projects/src/workflow.rs`, make the runtime-port test assert that the same derived
     `DispatchId` recorded by `ProjectInputDispatched` is carried in `ProjectRuntimeDelivery`.
2. Preserve the typed provenance before provider I/O.
   - In `crates/hq-projects/src/workflow.rs`, add `dispatch_id: DispatchId` to
     `ProjectRuntimeDelivery` and populate it from the already-derived dispatch identity. Preserve
     the established delivery-digest algorithm because the dispatch is deterministically derived
     from fields it already covers; compare retained attribution separately so upgrades do not
     invalidate an in-flight request identity.
   - In `crates/hq-harness/src/supervisor.rs` (and its re-export in
     `crates/hq-harness/src/lib.rs`), introduce a plainly named `HarnessProjectDelivery` value with
     `project_id`, `dispatch_id`, `assignment_id`, `thread_id`, and `sequence`; add
     `project: Option<HarnessProjectDelivery>` to `HarnessDeliveryRecord`. The submission ID remains
     the accepted input identity and the record's agent/provider/session fields remain the rest of
     the captured assignment binding. `None` means provenance was not retained, never "direct" or
     "best match."
   - In `crates/hq-node/src/harness_component.rs`, construct `Some(HarnessProjectDelivery)` from the
     exact `ProjectRuntimeDelivery` and compare it in `same_project_delivery`.
3. Make the operational store schema upgradeable and lossless.
   - In `crates/hq-store/src/harness.rs`, add a storage-owned optional project-attribution value and
     carry it on `StoredHarnessDelivery` without coupling the store to `hq-harness`.
   - In `crates/hq-store/src/database.rs`, bump the schema version, add nullable checked columns for
     project, dispatch, assignment, thread, and positive sequence to `harness_deliveries`, and add a
     narrowly scoped, transactional v1-to-v2 migration before ordinary schema verification. Accept
     only HQ's exact application ID/version/marker; update `user_version` only after all column
     additions succeed; continue rejecting unknown, partial, or future schemas.
   - In `crates/hq-store/src/database/harness.rs`, encode all attribution columns together, decode
     either all fields or none, reject partial/corrupt attribution, and include attribution in stable
     delivery equality and every load query. Existing v1 rows migrate to all-NULL attribution.
   - In `crates/hq-store/src/actor.rs` and `crates/hq-store/src/lib.rs`, adjust exposed storage types
     or actor messages only where exhaustiveness requires it; do not add a second write path.
4. Keep the node adapter exact and test the conversion seam.
   - In `crates/hq-node/src/harness_store.rs`, map the optional attribution field in both directions
     and add focused mapping coverage so no project field can be silently dropped again.
   - Update delivery fixtures in `crates/hq-harness/src/supervisor.rs`,
     `crates/hq-testkit/tests/supervisor_recovery.rs`, and any compiler-identified constructors to
     use `None` for genuinely provider-neutral test deliveries and complete attribution for project
     deliveries.
5. Verify with `cargo fmt --all --check`,
   `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`, and
   `cargo test --locked --workspace --all-targets --all-features`. Pay particular attention to
   SQLite migration/reopen tests and supervisor response-loss/idempotency tests.

#### Risks and decisions

- HQ currently has no schema migration path: `inspect_existing` rejects any version other than the
  current one. The migration must run before `verify_schema`, be limited to the known v1 schema, and
  stay crash-atomic. A generic migration framework is out of scope.
- Old `harness_deliveries` rows never stored enough evidence to populate these fields safely. This
  task preserves them as `None`; the following reconciliation task may attach provenance only when
  canonical dispatch evidence proves one exact match.
- Provider output can arrive after assignment handoff. Provenance is therefore immutable delivery
  evidence and must never be recomputed from the project's current assignment.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

### Canonically attribute new project runtime output and activity

HQ now resolves normalized runtime events through the exact retained agent/operation delivery before
authoring. Complete project deliveries produce home-linear `ProjectOutputRecorded` facts and FCT-022
activity with typed project, dispatch, captured assignment, and thread provenance; direct events keep
their historical shapes, while ambiguity, runtime mismatches, and unattributed legacy rows fail
closed. The protocol extension omits absent attribution so historical signed activity re-encodes
byte-for-byte. Conversation reduction now validates the cited dispatch and agent source and projects
project output with its immutable thread. Storage, restart recovery, reducer, migration, protocol,
and node tests cover exact replay and conflicts.

### Original plan entry

### Author canonical project output and activity for attributed deliveries

Use retained delivery provenance for all new runtime events. A project-bound output must author the
existing `ProjectOutputRecorded` fact, and project-bound activity must carry immutable project and
dispatch provenance in a new append-only canonical family. Direct-agent events retain their current
asynchronous-message and provider-session activity facts. Missing or conflicting delivery
attribution must fail closed rather than silently creating direct conversation history.

#### Implementation plan

1. Start with failing end-to-end persistence and reducer tests.
   - Extend `crates/hq-node/src/harness_persistence.rs` tests with a complete project fixture and
     assert that an attributed output authors `ProjectOutputRecorded` with the exact project,
     dispatch, captured assignment binding, selected thread, provider correlation, project mailbox,
     and `ProjectOutput` purpose. Assert attributed activity authors the new project-activity family
     with the same provenance; direct events must remain byte-for-byte in their existing families.
   - Extend `crates/hq-testkit/tests/conversation_reduction.rs` and
     `crates/hq-testkit/tests/project_reduction.rs` so valid project output/activity survive shuffled
     arrival and late output after handoff, while mismatched dispatch, assignment, thread, source,
     operation, and duplicate stable identities fail closed.
   - Extend `crates/hq-testkit/tests/supervisor_recovery.rs` to prove combined output/activity,
     partial checkpoint retry, response loss, and restart all pass the same retained attribution
     exactly once to persistence.
2. Add an append-only semantic contract for project activity.
   - In `crates/hq-domain/src/fact_catalog.rs` and `crates/hq-domain/src/semantic_fact.rs`, append
     `FCT-049 ProjectActivityRecorded`. Carry `project_id`, `dispatch_id`, captured
     `AssignmentBinding`, `thread_id`, and all existing normalized activity fields. Keep FCT-022
     unchanged so historical signatures and direct activity remain valid; make FCT-049
     canonical-compacted-view with installation-private scope and intrinsic nonzero/bounded values.
   - Add the exact closed DTO and family code in `crates/hq-protocol/src/dto/model.rs`, plus complete
     author/decode/semantic conversions in `author.rs`, `decode.rs`, and `semantic.rs`. Update
     `crates/hq-protocol/tests/semantic_conversion.rs`, catalog tests, DTO vectors, and
     `crates/hq-testkit/src/payloads.rs` so every family remains exhaustively executable.
   - In `crates/hq-reducer/src/conversation.rs`, normalize FCT-022 and FCT-049 through shared activity
     accessors. Require the project activity's exact projected `ProjectInputDispatched` parent and
     captured provenance, reuse source/sequence collision checks, and project/render it through the
     existing bounded activity model without discarding its canonical fact identity. Include
     `ProjectOutputRecorded` in message projection with its payload thread ID.
3. Resolve event attribution at the neutral supervisor boundary.
   - Add `delivery_for_operation(agent_id, operation_id)` to `HarnessStatePort` in
     `crates/hq-harness/src/supervisor.rs`; implement a bounded exact query in
     `crates/hq-store/src/database/harness.rs`, expose it through `database.rs` and `actor.rs`, and
     map it in `crates/hq-node/src/harness_store.rs`. More than one retained delivery for the same
     agent/operation is an explicit identity conflict.
   - Change `HarnessPersistencePort::persist_output` and `persist_activity` to receive an optional
     borrowed `HarnessProjectDelivery`. Before persistence, `persist_one` resolves the event's exact
     operation, verifies agent/provider/session against the retained delivery, passes `Some` only
     for complete provenance, returns an error for a matching unattributed legacy row, and passes
     `None` only when no delivery exists. Update neutral fake ports and fixtures exhaustively.
4. Plan the two canonical fact shapes from transaction-consistent state.
   - In `crates/hq-application/src/harness.rs` and `lib.rs`, add a project authoring authority/request
     that binds the current project head, project-home installation root, active account membership,
     exact dispatch fact, agent mailbox, project mailbox, and captured assignment. Branch
     `plan_harness_output` to build `ProjectOutputRecorded` with account scope, project mailbox,
     `ProjectOutput` purpose, and exact correlation; add `plan_project_harness_activity` for FCT-049
     without changing the direct planners.
   - In `crates/hq-node/src/harness_persistence.rs`, resolve that authority only by
     `ProjectProjectionKey::{Project,Dispatch}` and exact retained fields. Include the current head
     and dispatch fact as causal evidence so output remains valid after handoff; reject missing,
     conflicted, stale, or mismatched evidence. Keep stable command/digest derivation idempotent and
     preserve direct authoring unchanged.
5. Update `docs/protocol/payload-mapping-v1.md`, `docs/rust/semantic-facts.md`,
   `docs/rust/semantic-fact-catalog.md`, `docs/rust/project-model.md`, `docs/rust/storage.md`, and
   `docs/harnesses.md`. Raise the canonical-family storage bound to 49. Verify with
   `cargo fmt --all --check`, strict locked workspace Clippy, the locked all-target/all-feature
   workspace suite, and the repository's protocol/spec consistency tests.

#### Risks and decisions

- FCT-022 cannot safely gain a field: historical signed payloads must decode and re-encode exactly.
  An appended FCT-049 preserves compatibility and makes project attribution explicit.
- Project output is a project-home-linear transition. Author against the transaction's current head
  while also citing the immutable originating dispatch, so late output is retained without looking
  up the current assignment. Retry must re-plan after concurrent project progress rather than reuse
  a stale head.
- A provider operation with a retained but unattributed legacy delivery is not a direct message.
  This task fails it closed; the following task repairs legacy rows and already-authored output.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-30 — Project exchanges grouped by initiating thread

Added a typed project/thread conversation identity across reduction, rebuildable persistence, the
local API v1, and the node client. Project input, its correlated agent output, and attributed
activity now share one canonical conversation, while independently initiated messages in the same
project remain separate. The current pre-release storage and local API schemas remain version 1;
older unshipped layouts are rejected rather than migrated.

Regression coverage uses the reported two-message corpus and proves arrival-order invariance,
independent paging, repair/reopen persistence, canonical DTO conversion, stable TUI identity, and
closed key digests. Conversation, storage, event, API, and harness documentation now describe
project/thread identity as the grouping boundary. Formatting, strict workspace Clippy, focused
cross-layer tests, and the complete locked all-target/all-feature workspace suite pass.

### Original plan entry

### Group project exchanges by their initiating conversation

- Add a cross-layer regression first for the exact reported corpus: two project inputs (`Let's have
  a conversation.` and `Let's have another conversation.`), the Codex response `Absolutely. What’s
  on your mind?`, and its completed activity currently render as two `Thread <hex>` rows plus one
  `codex · <provider-session>` row. Require two project conversations: the first input, its Codex
  response, and the response's activity together in one conversation; the independently initiated
  second input in another conversation. Every entry must appear exactly once in canonical order,
  with no residual project-associated raw-Thread or provider-session row. Prove the same grouping
  under shuffled arrival, rebuild, duplicate persistence, response loss, reconnect, handoff, and
  ambiguous concurrent bindings.
- Add a typed project-exchange identity, such as
  `ConversationKey::ProjectThread { project_id, thread_id }`, throughout the reducer/application
  query boundary, rebuildable store index and page query, local API DTOs/conversion, node client,
  and TUI model. A newly initiated project message must create or retain that stable exchange key;
  replies and correlated agent output/activity must join it. Starting another conversation for the
  same project must create another key. Direct-agent and non-project conversations retain their own
  typed identities; never merge merely by project ID, content, display name, current assignment,
  provider/session coincidence, or row position. Thread IDs remain technical evidence, not ordinary
  Inbox labels.

#### Implementation plan

1. Add the regression before changing grouping. In
   `crates/hq-testkit/tests/conversation_reduction.rs`, build the exact two-input corpus with Alice's
   correlated final answer and completed activity, reduce representative arrival permutations, and
   assert two `ConversationKey::ProjectThread` orders: the first input/output/activity together and
   the second input alone. Assert that neither a raw `Thread` nor `ProviderSession` order retains any
   project-associated entry. Extend the store query contract to persist, rebuild, reopen, and page a
   project-thread key without duplicating entries.
2. In `crates/hq-reducer/src/conversation.rs`, add
   `ConversationKey::ProjectThread { project_id, thread }`. Select it before ordinary address or
   provider-session grouping whenever a projected message has typed `project_id`; use the
   `MessageView`'s derived/declared thread. Route `HarnessActivityRecorded` with typed project
   attribution to the same `(project_id, thread_id)` key. Preserve the existing exact Thread and
   ProviderSession rules for non-project messages and activity. Keep ordering solely on the
   canonical presentation comparator so arrival order, reconnect, duplicate persistence, assignment
   handoff, and provider-session changes cannot split or reorder an exchange.
3. Carry the closed key through rebuildable persistence without a compatibility migration or schema
   version bump. Update `crates/hq-store/src/snapshot.rs` hashing and
   `crates/hq-store/src/database/repair.rs` key encoding, exact-row validation, loading, and digesting;
   change the current pre-release `reduction_conversation_keys` definition in
   `crates/hq-store/src/database.rs` in place to store a project ID and a third closed key kind.
   Existing local databases may be discarded. Extend corruption and pagination tests so a key digest
   cannot alias a direct thread/provider-session key and a cursor cannot cross project exchanges.
4. Extend local API v1 in place—no protocol-version bump—in
   `crates/hq-local-api/src/protocol/v1.rs` and `crates/hq-local-api/src/conversion.rs` with the exact
   project/thread IDs, exhaustive validation/conversion, canonical JSON round trips, and server
   routing coverage. Update `crates/hq-node/src/tui_client.rs` and its tests to retain a stable
   full-ID project-exchange identity for requests and selection. Use only a temporary plain
   `Project conversation` presentation label here; authoritative names and message previews belong
   to the following Inbox-summary task.
5. Update `docs/rust/conversation-model.md`, `docs/rust/storage.md`,
   `docs/protocol/local-api-v1.md`, and any now-stale provider-session wording in `docs/events.md` to
   describe project/thread identity as the grouping boundary and provider/session/operation as
   provenance within the exchange. Run focused reducer, store, local-API, and node tests first, then
   formatting, strict Clippy, and the complete locked workspace test suite.

Risks and invariants:

- A project ID alone is intentionally insufficient: independently initiated messages for one
  project remain separate exchanges, while replies reuse the selected exchange's thread.
- Only signed typed `project_id`/`thread_id` or `ProjectActivityAttribution` may select this key.
  Never infer it from content, an agent's current assignment, a provider session, or row adjacency.
- Project output/activity whose dispatch or binding is ambiguous remains invalid/conflicted in the
  existing reducers and therefore cannot manufacture or contaminate an Inbox conversation.
- This is a pre-release layout change. Do not add legacy-row repair, schema migration, dual decoding,
  or a new database/local-API version.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.

## 2026-08-30 — Human-readable, selection-driven Inbox master/detail workspace

Added typed personal/direct/project conversation presentation context and bounded sanitized previews
to the authoritative summary path, local API v1, and node mapper without changing schema or protocol
versions. Inbox rows now use human titles and message snippets while ambiguous names degrade to an
unnamed-participant label without exposing internal IDs.

Inbox selection now eagerly and replaceably loads its conversation, ignores stale completions,
preserves list focus, and reuses an in-flight preview when the user enters it. Wide and compact
layouts keep a persistent conversation region with divider-only hierarchy, explicit loading and
diagnostic states, and one-level `h`/Left navigation back through the Inbox list. Pure-model,
render, cross-layer, and installed pseudoterminal coverage verifies the behavior, and the TUI
documentation describes the new workspace. Formatting, strict workspace Clippy, and the complete
locked all-target/all-feature workspace suite pass.

### Original plan entry

### Make Inbox a human-readable, selection-driven master/detail workspace

- Separate stable conversation identity from row presentation. Extend the bounded conversation
  summary/read model with typed project and participant context plus a sanitized, clipped preview of
  the conversation's first meaningful line (falling back to the latest meaningful line when the
  opener is unavailable). Render human titles such as `Project name · Alice` for project work and
  `Me and Alice` for direct conversations, with the preview as secondary text. The two conversations
  from the reported corpus must therefore be distinguishable by `Let's have a conversation.` and
  `Let's have another conversation.` without exposing IDs. Do not use `Thread <hex>`, raw mailbox
  IDs, provider namespaces, or provider-session UUIDs as ordinary titles; retain exact values in
  technical details and use a plain unnamed-participant fallback when authoritative naming is
  unresolved.
- Make Inbox selection drive conversation loading. Reconciliation of the first nonempty Inbox
  snapshot must select a stable conversation and request its first page automatically; `j`/`k` or
  Up/Down in the Inbox list must immediately request the newly selected row. A newer selection may
  supersede an older pending preview, stale completions must be inert, rapid movement must not show
  the wrong page, and a completed preview must retain list focus rather than stealing conversation
  focus. Preserve the selected project conversation and logical message anchor across refresh,
  resize, reconnect, and authoritative reorder.
- Always render the Inbox conversation region, including bounded loading, empty, unavailable, and
  selected-diagnostic states; do not expand the list to full width while no page is loaded and do
  not require Enter to make the pane appear. Enter or `l`/Right moves focus into the already visible
  selected conversation. `h`/Left moves one hierarchical level at a time—drafting pane to
  conversation, conversation to Inbox list, and Inbox list to top-level navigation—rather than
  jumping from the conversation to `Inbox / Sent / Archived / Agents / Projects`. Apply the same
  back-to-list rule before section changes in compact layouts, and keep `j`/`k` scoped to the list
  or conversation that visibly owns focus.
- Render the wide Inbox as adjacent navigation, conversation-list, and conversation panes separated
  by single vertical dividers. Replace the conversation's `Block::bordered()` rectangle with an
  internal heading and a focused/unfocused left divider matching the navigation/list boundary; draw
  no top, bottom, or right box lines. Use one unboxed separator and an explicit back path in the
  compact stacked layout, preserving useful rows for messages instead of spending them on chrome.

Cover human labels and previews, eager selection loading, rapid stale-load replacement, stable
selection anchors, focus hierarchy, divider-only responsive layout, pure-model rendering, and
installed PTY navigation. Update `docs/rust/tui.md` so Inbox is documented as a persistent
master/detail workspace rather than a list that conditionally opens a modal-like rectangle.

#### Implementation plan

1. Extend the authoritative conversation-summary contract before changing presentation.
   - In `crates/hq-application/src/snapshot.rs`, add a closed presentation context that distinguishes
     personal notes, direct conversations, and project conversations. Retain exact project,
     participant-agent, and participant-mailbox identities as typed values while making project and
     participant names optional when authoritative resolution is ambiguous. Add a bounded optional
     one-line preview to `ConversationSummary` and `ClientProjection::Conversation`.
   - In `crates/hq-store/src/database.rs`, derive this context from the same serialized reducer
     snapshots used for the conversation index: resolve project names by exact project ID; resolve
     agent names only from singular agent/name/mailbox evidence; prefer a project exchange's
     historical output or dispatch participant and use the current singular assignment only when
     the exchange has no historical agent evidence. Recognize the local-human counterparty as a
     personal note. Never infer identity from provider prose, row order, or content.
   - Derive the preview deterministically from reducer presentation order. Prefer the first
     meaningful message line, fall back to the latest meaningful message when the opener is
     unavailable, collapse control/line-breaking whitespace, and clip on a UTF-8 boundary to the
     short-text byte bound. Keep counts and exact keys unchanged. Add store/application regression
     coverage for the reported two project exchanges, direct named/unnamed participants, personal
     notes, ambiguous names, multiline/control content, Unicode clipping, shuffled ingestion,
     repair, and reopen.
2. Carry presentation context through local API v1 in place and map it once at the node boundary.
   - Add closed `ConversationContextDto` and participant/project DTOs plus `preview` to
     `SnapshotItem::Conversation` in `crates/hq-local-api/src/protocol/v1.rs`; update exhaustive
     application conversion and strict validation in `conversion.rs` without a protocol-version
     bump. Bound every optional display string and reject incoherent project/direct/personal
     combinations. Extend canonical JSON, snapshot conversion, and server-session tests.
   - In `crates/hq-node/src/tui_client.rs`, continue deriving the stable row/request ID only from
     `ConversationKeyDto`, but derive ordinary titles only from the typed context: `Project · Alice`,
     `Me and Alice`, or `Personal notes`, with `unnamed participant` as the conflict-safe fallback.
     Put the sanitized preview in `UiRow.detail`, falling back to a plain count only when no message
     preview exists. Assert that raw thread, mailbox, provider, session, project, and agent IDs never
     enter ordinary row title/detail text.
3. Make Inbox selection own replaceable preview loading in `crates/hq-tui/src/model.rs`.
   - Add failing model tests showing that the first nonempty Inbox snapshot selects its stable first
     conversation and emits a first-page load without activation; moving with Up/Down or `j`/`k`
     immediately clears the old preview and emits a new exact-row load. Let a newer selection
     replace the pending request identity so late completions are inert; keep page-row mismatch
     validation for the current request.
   - Distinguish eager preview loads from explicit entry. An eager completion must retain the
     existing list/navigation focus, while Enter or `l`/Right on the selected loaded conversation
     moves to conversation focus. Activation during an eager load records the entry intent without
     showing another row. Preserve the selected row and logical message anchor across authoritative
     reorder, refresh, resize, reconnect, and same-row reload; only a genuinely removed or newly
     selected row clears the old page.
   - Give conversation loading/failure evidence row scope instead of relying only on the global last
     failure, so the renderer can distinguish loading, loaded-empty, selected diagnostic,
     unavailable/failed, and no-conversation states without showing stale content.
4. Make focus traversal hierarchical and consistent at every supported width.
   - Replace the current left/`h` jump with one-level transitions: conversation to Inbox list,
     Inbox list to top-level navigation, and only then compact top-level section movement. Make
     Right/`l` and Enter enter the already visible conversation from the list; keep top-level
     navigation, list movement, and conversation-entry movement scoped to the focus that visibly
     owns them. Preserve Tab/Shift-Tab cycling without allowing an absent conversation to become a
     focus target.
   - Extend pure-model tests across wide and compact widths for `h`/`l`, arrow keys, Enter, rapid
     selection, diagnostics, empty Inbox, section changes, stale effects, and refresh/reconnect
     anchor preservation. Update footer/help copy to describe `back to Inbox`, `open conversation`,
     and top-level section movement in the actual focus state.
5. Replace conditional boxed rendering with a persistent Inbox master/detail composition in
   `crates/hq-tui/src/render.rs`.
   - Wide layout must always reserve adjacent navigation, conversation-list, and conversation
     regions. Give the conversation region one left divider whose focused/unfocused role matches
     the navigation divider, render an internal `Conversation` heading and paging state, and draw no
     top, bottom, or right borders. Compact layout must always use a bounded stacked list/detail
     composition with one unboxed separator and visible back guidance.
   - Render loading, empty, unavailable, failed, and selected-diagnostic states inside the persistent
     detail region. Keep reducer entry order and technical disclosure unchanged once loaded, and
     calculate entry capacity from the divider/header layout rather than the removed bordered box.
     Add style-aware buffer assertions and update wide/compact snapshots so no conversation box
     corners or horizontal border rows remain and useful message rows increase.
   - Extend `crates/hq-node/tests/unix_tui_terminal.rs` with an installed Inbox scenario proving
     eager detail visibility, list-owned movement, and `h` returning from conversation to the Inbox
     before top-level navigation. Update `docs/rust/tui.md`, run focused application/store/local-API/
     node/TUI suites, formatting, strict locked workspace Clippy, and the complete locked
     all-target/all-feature workspace suite.

Risks and invariants:

- Presentation metadata is disposable authoritative read-model data, not conversation identity.
  Names and previews may change after repair or new evidence without changing row IDs, cursors, or
  page membership.
- Ambiguous or stale agent/project evidence degrades to plain unnamed labels. It must never select a
  participant, merge conversations, retarget a page request, or leak an internal identifier.
- A superseded load may still finish in the shell; only the model's latest exact effect/row pair may
  mutate the visible pane. No cancellation or response-order assumption is required.
- The Inbox pane remains present even when empty or failed. Focus may enter Conversation only when
  the selected row has a loaded conversation; diagnostics and loading placeholders are not fake
  conversation history.
- This is a pre-release local API v1/read-model change. Do not add a version bump, legacy decoder,
  migration, or backwards-compatibility branch.

## Post-Plan Execution Steps

Execute these steps in order:

### Implement
Execute the plan above.

**Naming gate:** before creating any file, identifier, run-id, or env var, ask "would this name
make sense to someone who never read the plan?" If it encodes a sequence position (`Stage N` /
`Phase N` / `stepN`), rename it now — cheap before a checkpoint or downstream reference pins it.

### Verify

1. Run the project's build/lint command. Fix all warnings.
2. Run the project's test suite.
3. If tests fail, fix them before proceeding.
4. If test coverage for the new work is insufficient, add tests.

### Commit

Use Conventional Commits commit message style. If there are pre-existing modified files and they don't look harmful, go ahead and commit them, too.

### Update the plan file

Read the plan file at `/Users/wbbradley/src/hq/PLAN.md`. **Remove** the completed task entirely from the "Next Up" section — do not leave it in place with a [DONE] tag, strikethrough, or any other marker. The task and its related subsections should no longer appear in the plan file at all. The plan file should not have any sort of "Done" section. Then append a new entry to the completed file at `/Users/wbbradley/src/hq/COMPLETED.md` with two parts, in this order:

1. A brief summary, written now, of what was actually implemented.
2. The full text of the plan entry as it existed before work began, verbatim, not paraphrased, to preserve the original.

If upcoming plan items need modifications due to a change during this implementation then update those. If new future work items were discovered, add them. If the plan file or completed file is outside the source repository or is ignored, do not try to stage it; otherwise commit it with the other changes.
