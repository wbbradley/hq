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

### Present live and completed agent work clearly

Replace duplicate lifecycle/progress chrome with one informative live row at the bottom of an
active conversation, and make durable completed work recognizable without opening a generic
"Completed an item" details entry. Use typed activity data throughout; never infer operation
state, item family, command/output boundaries, or display policy from prose.

#### User-facing behavior

- While an agent turn is active, show exactly one live row as the final conversation entry. Before
  usable progress arrives it says "Agent is working…". Thereafter it shows the latest non-empty
  progress update for that typed operation. A newer update replaces the row in place instead of
  adding history.
- A correlated succeeded, failed, or interrupted agent-turn event removes the live row and replaces
  it with exactly one typed "Agent finished", error, or interruption row. Sequential and concurrent
  operations must remain isolated by full source/provider/session/operation identity.
- Durable completed items remain in conversation history with specific presentations:
  - Commands show the command, up to the first three output lines, and exit/failure state. Indicate
    omitted or truncated output and keep the complete bounded detail available in the inspector.
  - File changes summarize the changed paths and count without dumping diffs into ordinary history.
  - Tools show the retained server/tool or tool-family name.
  - Web searches show the retained query.
- Command output previews do not attempt secret detection or masking. Strip ANSI escape sequences,
  neutralize terminal control injection, and preserve safe line boundaries. Bound previews by line
  count and display width/bytes so hostile or malformed output cannot break the terminal layout.
- Progress and completed activity remain non-actionable. A reader following the tail stays on the
  live/replacement row; a reader who scrolled upward keeps their logical anchor through refresh or
  replacement. Initial load, continuation pages, refresh, reopen, and repair must not resurrect
  stale progress or omit the current live tail.

#### Typed data and presentation policy

- Define and document the provider-neutral distinction between `AgentTurn` (authoritative turn
  lifecycle), `Progress` (replaceable item-level telemetry), and durable completed-item families.
  Preserve canonical progress facts, deterministic projections, exact bounded diagnostic content,
  and the 200-progress-record per-session retention contract even when ordinary history consolidates
  them.
- Add a closed completed-item presentation type for at least command, file change, tool, and web
  search. Carry separately bounded command, output, exit code, changed paths/diffs, tool name, and
  query fields from `hq-codex` normalization through the harness/domain fact, protocol DTOs, reducer
  `ActivityView`, projection persistence, application entries, local API, node mapping, and TUI.
  Do not parse the existing flattened activity content downstream. This is pre-release schema work;
  update the current schema in place without a compatibility layer.
- Do not expand persistence to MCP/dynamic/collaboration tool arguments or results, web-search
  results, process IDs, or other provider payload that HQ does not already retain. Keep exact
  provider correlation on the presentation boundary and keep every new field explicitly bounded
  with truncation evidence.
- Select the latest canonical `Progress` winner across item keys for each active typed operation.
  Fall back to the running `AgentTurn` row before a useful progress update exists, and remove the
  transient row when terminal `AgentTurn` evidence exists. Do this below the page-local TUI so page
  boundaries cannot duplicate, omit, or revive transient rows.
- Expose a stable, pagination-safe presentation tail (or an equivalent typed aggregate) from the
  reducer/application/store conversation query while retaining one canonical comparator for durable
  entries. Remove or replace `move_running_agent_turns_to_tail`; presentation must not scan labels or
  rely on page-local adjacency.
- Replace the node mapper's newline-destroying activity sanitizer with terminal-safe handling for
  structured fields: remove ANSI escape sequences, neutralize unsafe controls, preserve intended
  newlines, and retain exact bounded detail for explicit technical disclosure.

#### Tests and documentation

- Add adapter normalization tests for structured command/output/exit data, multiline commands,
  empty output, failure, UTF-8-boundary truncation, file changes, MCP/dynamic/collaboration tools,
  and web search. Prove that excluded provider fields are not persisted.
- Add domain/protocol/reducer/store tests covering multiple progress item keys in one operation,
  running fallback, latest-progress replacement, success/failure/interruption, sequential and
  concurrent operations, reverse arrival, conflicts, rebuild, retention, and page boundaries.
  Retained facts/projections must remain deterministic while ordinary presentation contains only
  the selected live state.
- Add application/local-API/store paging coverage for conversations longer than both the TUI's
  initial page and the progress-retention limit. A current live tail must be available without
  duplicating it in continuation pages.
- Add node and TUI mapper/model/render/installed-terminal tests for the single bottom live row,
  replacement by terminal state, multiline three-line command previews, ANSI/control removal,
  file/tool/search summaries, status and truncation markers, measured row heights, technical
  details, stable selection, follow-tail behavior, refresh, pagination, and sequential turns.
  Assert that duplicate "Work in progress…" and generic "Completed an item" labels are absent when
  typed data is available.
- Update the conversation, TUI, inbox surface, acceptance, behavior-ledger, semantic-fact, storage,
  harness, and protocol documentation to describe lifecycle versus progress semantics,
  retained-versus-presented activity, completed-item fields, tail pagination, command-output
  disclosure, and terminal-safety rules.
- Run formatting, strict workspace Clippy/build checks, focused adapter/reducer/store/API/TUI tests,
  the installed PTY regression suite, and the complete workspace test suite.

#### Acceptance criteria

- One and only one active-turn row is visible at the conversation bottom, and the latest progress
  update replaces it in place.
- Terminal lifecycle and live progress never coexist for the same operation; terminal state
  replaces live state deterministically without affecting another operation.
- Commands show typed command text, up to three terminal-safe output lines, exit/failure state, and
  omission/truncation cues without downstream prose parsing or secret masking.
- File, tool, and web-search entries are recognizable from closed typed data; a generic completed
  label is used only as an honest fallback for an explicitly retained unknown family.
- ANSI/control content cannot affect terminal behavior or layout. Pagination, refresh, reopen,
  repair, long histories, sequential/concurrent turns, and non-tail selection preserve the same
  logical presentation.
- Canonical facts, bounded progress retention, durable completed-item history, activity
  non-actionability, and typed failure/interruption semantics remain intact.

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
