# TUI themes

HQ resolves one complete theme when the TUI starts. It does not reload theme files while running;
restart `hq` after editing or selecting a theme.

List every bundled and discovered choice, including invalid files, with:

```sh
hq config themes
```

Select a bundled or user theme by name, select one explicit file, or return to automatic behavior:

```sh
hq config set theme gruvbox-dark-medium
hq config set theme /absolute/path/to/my-theme.toml
hq config set theme none
```

Named user themes are `.toml`, `.yaml`, or `.yml` files in
`$XDG_CONFIG_HOME/hq/themes`; when `XDG_CONFIG_HOME` is unset, HQ uses
`~/.config/hq/themes`. The selector is the filename without its extension. Theme files must be
regular, non-symlink files and, on Unix, must not be group- or world-writable. HQ reports duplicate
names, reserved built-in names, unsafe files, malformed definitions, and unresolved parents rather
than guessing.

With no explicit selection, HQ uses `gruvbox-dark-medium`. A nonempty `NO_COLOR` changes that
automatic choice to `no-color`; an explicit configured choice wins. The bundled choices are:

- `terminal`
- `no-color`
- `gruvbox-dark-hard`, `gruvbox-dark-medium`, `gruvbox-dark-soft`
- `gruvbox-light-hard`, `gruvbox-light-medium`, `gruvbox-light-soft`

The Gruvbox palettes come from the MIT-licensed Tinted Theming schemes repository at commit
`fdca32a0d14ec80ad83a78a9ccb85592ca6cb9e1`. Their embedded metadata retains attribution to Dawid
Kurek and morhetz.

## Native TOML format

A native theme optionally names a parent, provides local palette aliases, and overrides any subset
of HQ's semantic roles. Unspecified roles come from the resolved parent; the default parent is
`terminal`. Quote role keys because their dots are part of one semantic key, not TOML nesting.

```toml
name = "My calm dark theme"
author = "Your Name"
inherits = "gruvbox-dark-medium"

[palette]
paper = "#181818"
surface = "#242424"
ink = "#e8e8e8"
selected = "#3a4a5a"

[styles."ui.screen"]
fg = "ink"
bg = "paper"

[styles."ui.text"]
fg = "ink"
bg = "paper"

[styles."ui.modal.surface"]
fg = "ink"
bg = "surface"

[styles."ui.selection.focused"]
fg = "ink"
bg = "selected"
modifiers = ["bold"]

[styles."ui.selection.unfocused"]
fg = "ink"
bg = "surface"

[styles."ui.cursor"]
fg = "paper"
bg = "ink"
underline = "selected"
modifiers = ["reversed"]
```

`fg`, `bg`, and `underline` accept:

- `reset`
- `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `gray`, `dark-gray`,
  `light-red`, `light-green`, `light-yellow`, `light-blue`, `light-magenta`, `light-cyan`, or
  `white`
- an indexed terminal color from `ansi:0` through `ansi:255`
- a six-digit RGB value such as `#d79921`
- a name from the file's `[palette]`

The optional `modifiers` list replaces the inherited role's modifier set. Its values are `bold`,
`dim`, `italic`, `underlined`, `slow-blink`, `rapid-blink`, `reversed`, `hidden`, and
`crossed-out`.

Every configurable role is listed below. A resolved theme always has a style for every role.

| Role | Element |
| --- | --- |
| `ui.screen` | Full frame, ordinary defaults, and blank cells |
| `ui.text` | Ordinary interface text |
| `ui.text.muted` | Explanatory and de-emphasized text |
| `ui.text.technical` | IDs and technical evidence |
| `ui.heading` | Section headings |
| `ui.accent` | Primary interactive accent |
| `ui.selection.focused` | Selected item in the focused control |
| `ui.selection.unfocused` | Selected item outside the focused control |
| `ui.border.focused` | Focused control border |
| `ui.border.unfocused` | Unfocused control border |
| `ui.modal.surface` | Cleared dialog and help-overlay surface |
| `ui.modal.border` | Dialog border |
| `ui.modal.title` | Dialog title |
| `ui.header.badge` | HQ header badge |
| `ui.input` | Editable and choice input |
| `ui.input.field` | Padded surface of an unfocused one-line text field |
| `ui.input.field.focused` | Padded surface of the focused one-line text field |
| `ui.cursor` | Text insertion cursor |
| `ui.footer` | Ordinary key guidance |
| `ui.footer.success` | Successful completion guidance |
| `ui.footer.warning` | Warning and recovery guidance |
| `status.connection.ready` | Ready connection state |
| `status.connection.pending` | Connecting or reconnecting state |
| `status.connection.error` | Offline or incompatible state |
| `status.row.open` | Open row state |
| `status.row.waiting` | Waiting row state |
| `status.row.archived` | Archived row state |
| `status.row.attention` | Row needing attention |
| `status.success` | Successful inline feedback |
| `status.warning` | Warning inline feedback |
| `status.error` | Error inline feedback |
| `status.attention` | Strong attention feedback |

## Base16 import

HQ also reads the current Tinted Theming Base16 YAML shape locally and offline: `system`, `name`,
`author`, `variant`, and exactly `base00` through `base0F` under `palette`. A Base16 file becomes a
complete HQ theme using this mapping:

| Base16 color | HQ use |
| --- | --- |
| `base00` | Screen background |
| `base05` | Ordinary text |
| `base03` | Muted text |
| `base02` | Selections and secondary surfaces |
| `base08` | Errors |
| `base0A` | Warnings |
| `base0B` | Success |
| `base0D` | Accents and focus |

A native TOML theme may `inherits` a Base16 filename stem and then override individual semantic
roles. Base16 is palette interchange, not HQ's semantic role schema.

RGB colors are passed to the terminal exactly as requested. A terminal without truecolor support
may approximate them; HQ does not silently rewrite the theme. Prefer `terminal`, ANSI names, or
`ansi:N` when terminal-native colors are important. The `no-color` theme retains bold, dim, reverse,
labels, borders, and selection markers so focus and state remain understandable without color.
