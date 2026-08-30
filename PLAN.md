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
