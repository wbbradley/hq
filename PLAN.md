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

#### Implementation plan

1. Establish the field-rendering contract with failing tests in
   `crates/hq-tui/tests/render_snapshots.rs`: the label remains outside the control; a gap follows
   the colon; the surface fills the dialog's inner width; the empty requirement is right-aligned;
   focused, unfocused, and caret cells use independent semantic styles; filled, narrow, and Unicode
   values neither wrap nor corrupt a cursor boundary. Add focused model coverage only where the
   renderer needs a state that existing helpers cannot express.
2. Extend `UiThemeRole` in `crates/hq-tui/src/theme.rs` with focused and unfocused field-surface
   roles while retaining `ui.input` for choice rows. Give terminal, no-color, and Base16 themes
   complete defaults and cover their key inventory. Update the node's native-theme tests if the
   complete role catalog changes expected definitions.
3. Add `unicode-width` as an explicit workspace and `hq-tui` dependency in `Cargo.toml` and
   `crates/hq-tui/Cargo.toml`. Refactor the shared `text_field_line` path in
   `crates/hq-tui/src/render.rs` into a width-aware component that computes display-cell widths,
   clips a long value around the active caret, pads the whole remaining row, floats an empty
   requirement at the right edge, and composes cursor style over the selected field surface. Pass
   each modal's inner width through project, agent, search, and saved-conversation call sites.
4. Add failing project-workflow tests in `crates/hq-projects/tests/activation_dispatch.rs` proving a
   threadless activation rejects before assignment/runtime effects and an explicitly selected
   pending-input thread activates and dispatches normally. Move thread selection and persistence to
   the activation prerequisite boundary in `crates/hq-projects/src/workflow.rs`; accept an explicit
   thread only when it is historical or belongs to a pending input, and remove late post-runtime
   selection.
5. Expose exact accepted-input thread identity through the authoritative snapshot boundary. Extend
   `crates/hq-application/src/snapshot.rs`, `crates/hq-application/tests/project_snapshot.rs`,
   `crates/hq-local-api/src/protocol/v1.rs`, `crates/hq-local-api/src/conversion.rs`, and their
   protocol tests so `ProjectInput` carries its reducer-derived thread ID. Carry pending input
   message/thread/sequence records through `crates/hq-node/src/cli.rs`,
   `crates/hq-node/src/local_client.rs`, and `crates/hq-node/src/tui_client.rs` into `UiProject` in
   `crates/hq-tui/src/model.rs`, filtering out already-dispatched inputs at the node presentation
   boundary.
6. Write failing pure-model tests in `crates/hq-tui/tests/model.rs` for zero, one, and multiple
   available providers; automatic exact historical resume; move/handoff review; retained first
   instruction; matching accepted-input refresh; exact-thread activation; rejection/reconnect
   recovery; runnable refresh; and no duplicate send. Refactor guided state into explicit
   instruction-acceptance, accepted-input-refresh, and activation states. With one available
   provider, transition directly to the next meaningful state; with several, retain the provider
   picker; with none, retain the actionable unavailable screen. For a target without compatible
   history, collect and send the instruction, wait for the matching pending input, then activate or
   hand off on its exact thread and finally open that conversation without resending.
7. Extend `crates/hq-node/tests/unix_tui_terminal.rs` with the installed post-bootstrap-from-nothing
   `n` flow, using one provider and asserting that no `Start project work` confirmation or
   `project_activation_thread_missing` failure appears and that one instruction yields one runnable
   dispatch. Update any local-client, TUI-effect, fixture, codec, or snapshot tests made incomplete
   by the typed field additions.
8. Update `docs/tui-themes.md`, `docs/rust/tui.md`, and `docs/protocol/local-api-v1.md` for the field
   roles, full-width field behavior, single-provider skip, accepted-input correlation, and
   first-instruction ordering. Run formatting, architecture and protocol/spec consistency checks,
   focused crate tests, strict workspace Clippy, the locked all-target/all-feature workspace suite,
   build, and installed TUI terminal tests.

#### Risks and resolved questions

- Existing custom native themes remain compatible because new roles resolve from their inherited
  complete parent; unknown-role rejection remains strict.
- Display-cell clipping must not change the editor's byte-index cursor. Rendering will derive a
  visible window from valid UTF-8 boundaries while leaving the pure form state untouched.
- A returned message ID alone is not a thread identity. The flow will wait for a snapshot record
  that joins that exact accepted message to its reducer-derived thread before activation.
- Project input is accepted before activation by design and remains durably pending if setup is
  rejected or interrupted. Recovery must preserve that fact and must never author a replacement
  input implicitly.

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
