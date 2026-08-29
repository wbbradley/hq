# HQ

## Product direction

Design the TUI for people who have never seen HQ and do not know its internal vocabulary. Every
screen and dialog must make clear what the user is looking at, why HQ needs their input, what they
can do next, and what will happen afterward. Prefer user intentions and ordinary language over
authority, reducer, provider-session, assignment, thread, reconciliation, and other implementation
terms. Preserve exact technical evidence behind contextual details and recovery views.

Keep these user workflows distinct and composable:

- Projects define work and authoritative ownership of resources. Resource ownership is a core HQ
  concern; Git worktree creation and lifecycle management are not the product's center and should
  remain optional, progressively disclosed conveniences. Agents may eventually manage worktrees
  themselves.
- Agents are named workers that can be assigned to project work and contacted through
  conversations. Starting work should hide routine provider-session and assignment mechanics.
- Direct messaging, including future communication with other humans in the HQ network, remains a
  first-class path rather than an awkward special case of project work.
- Personal notes remain available without competing with the primary collaboration actions.

Never require a user to guess a valid identifier, namespace, state transition, or recovery command
when HQ already has enough typed information to present valid choices. Use progressive disclosure:
ordinary screens explain goals and next actions; details screens expose stable IDs, causal evidence,
provider/session identities, and recovery diagnostics.

## Next Up

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

### Add an extensible guided `New...` workflow

Provide a clear path from intent to conversation while keeping project work, direct messages, and
personal notes distinct.

- Make `n new` open a small launcher with `Work with an agent on a project`, `Send a direct message`,
  and `Write a personal note`. Retain expert shortcuts where useful. Structure direct-message target
  selection so future human peers can appear alongside other typed recipients without changing the
  project workflow or inventing provider sessions for humans.
- For project work, guide the user through selecting or creating a project, selecting or creating an
  unassigned agent, choosing a provider only when necessary, and composing the initial instruction.
  Show a compact review in user terms before any materially different assignment or handoff.
- Orchestrate the existing retry-safe project activation/session and input operations behind the
  workflow. Resume a compatible existing project conversation when selected; otherwise create the
  required scoped session and assignment, then dispatch the initial instruction exactly once.
- Explain exceptional choices in place: an agent assigned to another project requires an explicit
  handoff path; resource or assignment conflicts identify the competing project; rejected or
  uncertain setup retains the user's selections and draft for recovery.
- On success, open the resulting conversation with a visible context banner naming the project,
  agent, and provider. Do not expose the user to an empty Inbox, operation outcome dialog, or a
  separate unexplained managed-session step.
- Preserve direct agent sessions, direct messaging, notes, and project/resource administration as
  independent capabilities. The guided project path is a convenience over the domain model, not a
  replacement for HQ's broader collaboration model.

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
