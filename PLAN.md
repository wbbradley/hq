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
