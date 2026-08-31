//! Startup-only TUI theme discovery, parsing, and resolution.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::Read as _,
    path::{Path, PathBuf},
};

use hq_tui::{Base16Palette, UiTheme, UiThemeRole};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

use crate::ThemeSelection;

const MAX_THEME_BYTES: u64 = 65_536;
const MAX_THEME_FILES: usize = 256;
const MAX_INHERITANCE_DEPTH: usize = 8;
const MAX_METADATA_BYTES: usize = 256;
const DEFAULT_THEME_NAME: &str = "gruvbox-dark-medium";
const BUILTIN_BASE16: [(&str, &[u8]); 6] = [
    (
        "gruvbox-dark-hard",
        include_bytes!("../assets/themes/gruvbox-dark-hard.yaml"),
    ),
    (
        "gruvbox-dark-medium",
        include_bytes!("../assets/themes/gruvbox-dark-medium.yaml"),
    ),
    (
        "gruvbox-dark-soft",
        include_bytes!("../assets/themes/gruvbox-dark-soft.yaml"),
    ),
    (
        "gruvbox-light-hard",
        include_bytes!("../assets/themes/gruvbox-light-hard.yaml"),
    ),
    (
        "gruvbox-light-medium",
        include_bytes!("../assets/themes/gruvbox-light-medium.yaml"),
    ),
    (
        "gruvbox-light-soft",
        include_bytes!("../assets/themes/gruvbox-light-soft.yaml"),
    ),
];
const BUILTIN_NAMES: [&str; 8] = [
    "terminal",
    "no-color",
    "gruvbox-dark-hard",
    "gruvbox-dark-medium",
    "gruvbox-dark-soft",
    "gruvbox-light-hard",
    "gruvbox-light-medium",
    "gruvbox-light-soft",
];

/// Stable startup-theme failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiThemeErrorClass {
    /// No usable user configuration directory can be derived.
    PathUnavailable,
    /// A theme name, path, color, role, modifier, or metadata field is invalid.
    Invalid,
    /// A configured or inherited theme does not exist.
    NotFound,
    /// More than one user file claims the same theme name.
    Ambiguous,
    /// Theme inheritance contains a cycle or exceeds its bound.
    Inheritance,
    /// A theme file or directory is a symlink, non-file, or writable by other users.
    UnsafeFile,
    /// Theme bytes are malformed, non-UTF-8, unknown, or oversized.
    Malformed,
    /// Theme discovery or file reading failed.
    FileSystem,
}

/// One bounded actionable startup-theme failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiThemeError {
    class: TuiThemeErrorClass,
    subject: String,
}

impl TuiThemeError {
    fn new(class: TuiThemeErrorClass, subject: impl AsRef<str>) -> Self {
        Self {
            class,
            subject: bounded_subject(subject.as_ref()),
        }
    }

    /// Returns the stable failure class.
    pub const fn class(&self) -> TuiThemeErrorClass {
        self.class
    }

    /// Returns the bounded file, theme, or field that needs attention.
    pub fn subject(&self) -> &str {
        &self.subject
    }
}

impl fmt::Display for TuiThemeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.class {
            TuiThemeErrorClass::PathUnavailable => "theme configuration path is unavailable",
            TuiThemeErrorClass::Invalid => "theme value is invalid",
            TuiThemeErrorClass::NotFound => "theme was not found",
            TuiThemeErrorClass::Ambiguous => "theme name is ambiguous",
            TuiThemeErrorClass::Inheritance => "theme inheritance is invalid",
            TuiThemeErrorClass::UnsafeFile => "theme file is unsafe",
            TuiThemeErrorClass::Malformed => "theme file is malformed",
            TuiThemeErrorClass::FileSystem => "theme filesystem operation failed",
        };
        write!(formatter, "{message}: {}", self.subject)
    }
}

impl std::error::Error for TuiThemeError {}

/// Environment-derived inputs used for deterministic theme resolution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TuiThemeEnvironment {
    /// Explicit XDG configuration home, when set.
    pub xdg_config_home: Option<PathBuf>,
    /// Operating-system home directory fallback.
    pub home_directory: Option<PathBuf>,
    /// Exact `NO_COLOR` value; only a nonempty value is active.
    pub no_color: Option<OsString>,
}

impl TuiThemeEnvironment {
    /// Captures theme-related process environment once at startup.
    pub fn from_environment() -> Self {
        Self {
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            home_directory: std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            no_color: std::env::var_os("NO_COLOR"),
        }
    }

    /// Returns the platform user-theme directory.
    pub fn theme_directory(&self) -> Result<PathBuf, TuiThemeError> {
        let root = if let Some(root) = &self.xdg_config_home {
            root.clone()
        } else if let Some(home) = &self.home_directory {
            home.join(".config")
        } else {
            return Err(TuiThemeError::new(
                TuiThemeErrorClass::PathUnavailable,
                "$XDG_CONFIG_HOME or $HOME",
            ));
        };
        if !root.is_absolute() || root.as_os_str().is_empty() {
            return Err(TuiThemeError::new(
                TuiThemeErrorClass::PathUnavailable,
                root.to_string_lossy(),
            ));
        }
        Ok(root.join("hq/themes"))
    }

    fn no_color_requested(&self) -> bool {
        self.no_color
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    }
}

/// One built-in or user theme visible through `hq config themes`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuiThemeCatalogEntry {
    /// Exact selector accepted by `hq config set theme`.
    pub selector: String,
    /// Human-facing theme name from its metadata.
    pub name: String,
    /// Optional original author attribution.
    pub author: Option<String>,
    /// `built-in`, `toml`, or `base16`.
    pub source: String,
    /// Whether this entry is the configured or automatic active choice.
    pub active: bool,
    /// Bounded validation failure for a discovered but unusable user file.
    pub error: Option<String>,
}

/// Resolves one complete startup theme without changing terminal state.
pub fn resolve_tui_theme(
    selection: Option<&ThemeSelection>,
    environment: &TuiThemeEnvironment,
) -> Result<UiTheme, TuiThemeError> {
    if selection.is_none() && environment.no_color_requested() {
        return Ok(UiTheme::no_color());
    }
    let Some(selection) = selection else {
        return builtin_theme(DEFAULT_THEME_NAME)?.ok_or_else(|| {
            TuiThemeError::new(TuiThemeErrorClass::Invalid, "default built-in theme")
        });
    };
    let theme_directory = environment.theme_directory().ok();
    ThemeResolver::new(theme_directory).resolve_selection(selection)
}

/// Lists bundled and discovered user themes with validation results.
pub fn list_tui_themes(
    selection: Option<&ThemeSelection>,
    environment: &TuiThemeEnvironment,
) -> Result<Vec<TuiThemeCatalogEntry>, TuiThemeError> {
    let automatic = if selection.is_none() && environment.no_color_requested() {
        "no-color"
    } else {
        DEFAULT_THEME_NAME
    };
    let active = selection.map_or(automatic, ThemeSelection::as_str);
    let mut entries = BUILTIN_NAMES
        .into_iter()
        .map(|selector| {
            let theme = builtin_theme(selector)?.ok_or_else(|| {
                TuiThemeError::new(TuiThemeErrorClass::Invalid, "built-in inventory")
            })?;
            Ok(TuiThemeCatalogEntry {
                selector: selector.to_owned(),
                name: theme.name().to_owned(),
                author: theme.author().map(str::to_owned),
                source: "built-in".to_owned(),
                active: active == selector,
                error: None,
            })
        })
        .collect::<Result<Vec<_>, TuiThemeError>>()?;
    let theme_directory = match environment.theme_directory() {
        Ok(directory) => Some(directory),
        Err(_)
            if selection.is_none_or(|value| {
                !value.is_absolute_path() && is_builtin_name(value.as_str())
            }) =>
        {
            return Ok(entries);
        }
        Err(_) if selection.is_some_and(ThemeSelection::is_absolute_path) => None,
        Err(error) => return Err(error),
    };
    let resolver = ThemeResolver::new(theme_directory);
    entries.extend(resolver.catalog(active)?);
    if let Some(selection) = selection
        && !entries
            .iter()
            .any(|entry| entry.selector == selection.as_str())
    {
        entries.push(match resolver.resolve_selection(selection) {
            Ok(theme) => TuiThemeCatalogEntry {
                selector: selection.as_str().to_owned(),
                name: theme.name().to_owned(),
                author: theme.author().map(str::to_owned),
                source: if selection.is_absolute_path() {
                    theme_source(Path::new(selection.as_str()))
                } else {
                    "file"
                }
                .to_owned(),
                active: true,
                error: None,
            },
            Err(error) => TuiThemeCatalogEntry {
                selector: selection.as_str().to_owned(),
                name: file_stem(Path::new(selection.as_str()))
                    .unwrap_or_else(|| "invalid theme".to_owned()),
                author: None,
                source: "file".to_owned(),
                active: true,
                error: Some(error.to_string()),
            },
        });
    }
    Ok(entries)
}

struct ThemeResolver {
    theme_directory: Option<PathBuf>,
}

impl ThemeResolver {
    const fn new(theme_directory: Option<PathBuf>) -> Self {
        Self { theme_directory }
    }

    fn resolve_selection(&self, selection: &ThemeSelection) -> Result<UiTheme, TuiThemeError> {
        let mut stack = Vec::new();
        if selection.is_absolute_path() {
            let path = Path::new(selection.as_str());
            let name = file_stem(path).unwrap_or_else(|| "custom".to_owned());
            self.resolve_file(path, &name, 0, &mut stack)
        } else {
            self.resolve_named(selection.as_str(), 0, &mut stack)
        }
    }

    fn resolve_named(
        &self,
        name: &str,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Result<UiTheme, TuiThemeError> {
        if depth > MAX_INHERITANCE_DEPTH || stack.iter().any(|ancestor| ancestor == name) {
            return Err(TuiThemeError::new(TuiThemeErrorClass::Inheritance, name));
        }
        if let Some(theme) = builtin_theme(name)? {
            return Ok(theme);
        }
        if !valid_theme_name(name) {
            return Err(TuiThemeError::new(TuiThemeErrorClass::Invalid, name));
        }
        let path = self.find_named_file(name)?;
        stack.push(name.to_owned());
        let result = self.resolve_file(&path, name, depth, stack);
        stack.pop();
        result
    }

    fn resolve_file(
        &self,
        path: &Path,
        selector_name: &str,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Result<UiTheme, TuiThemeError> {
        if depth > MAX_INHERITANCE_DEPTH {
            return Err(TuiThemeError::new(
                TuiThemeErrorClass::Inheritance,
                selector_name,
            ));
        }
        let marker = format!("file:{}", path.to_string_lossy());
        if stack.iter().any(|ancestor| ancestor == &marker) {
            return Err(TuiThemeError::new(
                TuiThemeErrorClass::Inheritance,
                selector_name,
            ));
        }
        stack.push(marker);
        let bytes = read_theme_file(path)?;
        let result = match path.extension().and_then(|extension| extension.to_str()) {
            Some("toml") => {
                let source = std::str::from_utf8(&bytes).map_err(|_| {
                    TuiThemeError::new(TuiThemeErrorClass::Malformed, path.to_string_lossy())
                })?;
                let definition: NativeThemeDto = toml::from_str(source).map_err(|error| {
                    TuiThemeError::new(
                        TuiThemeErrorClass::Malformed,
                        format!("{}: {error}", path.to_string_lossy()),
                    )
                })?;
                self.resolve_native(definition, path, selector_name, depth, stack)
            }
            Some("yaml" | "yml") => parse_base16(&bytes, path),
            _ => Err(TuiThemeError::new(
                TuiThemeErrorClass::Invalid,
                format!("{}: expected .toml, .yaml, or .yml", path.to_string_lossy()),
            )),
        };
        stack.pop();
        result
    }

    fn resolve_native(
        &self,
        definition: NativeThemeDto,
        path: &Path,
        selector_name: &str,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Result<UiTheme, TuiThemeError> {
        let parent_name = definition.inherits.as_deref().unwrap_or("terminal");
        let mut theme = self.resolve_named(parent_name, depth + 1, stack)?;
        let palette = parse_palette(&definition.palette, path)?;
        for (key, style) in definition.styles {
            let role = UiThemeRole::from_key(&key).ok_or_else(|| {
                TuiThemeError::new(
                    TuiThemeErrorClass::Invalid,
                    format!("{}: styles.{key}", path.to_string_lossy()),
                )
            })?;
            let resolved = apply_style(theme.style(role), style, &palette, path, &key)?;
            theme = theme.with_style(role, resolved);
        }
        let name = definition.name.unwrap_or_else(|| selector_name.to_owned());
        validate_metadata(&name, "name", path)?;
        if let Some(author) = &definition.author {
            validate_metadata(author, "author", path)?;
        }
        let author = definition
            .author
            .or_else(|| theme.author().map(str::to_owned));
        Ok(theme.with_metadata(name, author))
    }

    fn find_named_file(&self, name: &str) -> Result<PathBuf, TuiThemeError> {
        let directory = self
            .theme_directory
            .as_ref()
            .ok_or_else(|| TuiThemeError::new(TuiThemeErrorClass::PathUnavailable, name))?;
        let files = discover_theme_files(directory)?;
        let matches = files
            .into_iter()
            .filter(|path| file_stem(path).as_deref() == Some(name))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Err(TuiThemeError::new(TuiThemeErrorClass::NotFound, name)),
            [path] if !is_builtin_name(name) => Ok(path.clone()),
            [_] => Err(TuiThemeError::new(
                TuiThemeErrorClass::Ambiguous,
                format!("{name}: reserved built-in name"),
            )),
            _ => Err(TuiThemeError::new(TuiThemeErrorClass::Ambiguous, name)),
        }
    }

    fn catalog(&self, active: &str) -> Result<Vec<TuiThemeCatalogEntry>, TuiThemeError> {
        let Some(directory) = &self.theme_directory else {
            return Ok(Vec::new());
        };
        let files = discover_theme_files(directory)?;
        let mut grouped = BTreeMap::<String, Vec<PathBuf>>::new();
        for path in files {
            let Some(name) = file_stem(&path) else {
                continue;
            };
            grouped.entry(name).or_default().push(path);
        }
        let mut entries = Vec::new();
        for (selector, paths) in grouped {
            let error = if is_builtin_name(&selector) {
                Some("name is reserved by a built-in theme".to_owned())
            } else if paths.len() > 1 {
                Some("more than one theme file has this name".to_owned())
            } else {
                None
            };
            if let Some(error) = error {
                entries.push(TuiThemeCatalogEntry {
                    selector: selector.clone(),
                    name: selector.clone(),
                    author: None,
                    source: "file".to_owned(),
                    active: active == selector,
                    error: Some(error),
                });
                continue;
            }
            let selection = ThemeSelection::new(selector.clone())
                .map_err(|_| TuiThemeError::new(TuiThemeErrorClass::Invalid, selector.as_str()))?;
            match self.resolve_selection(&selection) {
                Ok(theme) => entries.push(TuiThemeCatalogEntry {
                    selector: selector.clone(),
                    name: theme.name().to_owned(),
                    author: theme.author().map(str::to_owned),
                    source: theme_source(&paths[0]).to_owned(),
                    active: active == selector,
                    error: None,
                }),
                Err(error) => entries.push(TuiThemeCatalogEntry {
                    selector: selector.clone(),
                    name: selector.clone(),
                    author: None,
                    source: theme_source(&paths[0]).to_owned(),
                    active: active == selector,
                    error: Some(error.to_string()),
                }),
            }
        }
        Ok(entries)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeThemeDto {
    name: Option<String>,
    author: Option<String>,
    inherits: Option<String>,
    #[serde(default)]
    palette: BTreeMap<String, String>,
    #[serde(default)]
    styles: BTreeMap<String, StyleDto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StyleDto {
    fg: Option<String>,
    bg: Option<String>,
    underline: Option<String>,
    modifiers: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Base16Dto {
    system: String,
    name: String,
    author: String,
    variant: String,
    palette: BTreeMap<String, String>,
}

fn parse_base16(bytes: &[u8], path: &Path) -> Result<UiTheme, TuiThemeError> {
    let definition: Base16Dto = serde_saphyr::from_slice(bytes).map_err(|error| {
        TuiThemeError::new(
            TuiThemeErrorClass::Malformed,
            format!("{}: {error}", path.to_string_lossy()),
        )
    })?;
    if definition.system != "base16" || !matches!(definition.variant.as_str(), "dark" | "light") {
        return Err(TuiThemeError::new(
            TuiThemeErrorClass::Invalid,
            format!("{}: system or variant", path.to_string_lossy()),
        ));
    }
    validate_metadata(&definition.name, "name", path)?;
    validate_metadata(&definition.author, "author", path)?;
    if definition.palette.len() != 16 {
        return Err(TuiThemeError::new(
            TuiThemeErrorClass::Invalid,
            format!(
                "{}: palette must contain base00-base0F",
                path.to_string_lossy()
            ),
        ));
    }
    let mut colors = [Color::Reset; 16];
    for (index, color) in colors.iter_mut().enumerate() {
        let key = format!("base{index:02X}");
        let value = definition.palette.get(&key).ok_or_else(|| {
            TuiThemeError::new(
                TuiThemeErrorClass::Invalid,
                format!("{}: palette.{key}", path.to_string_lossy()),
            )
        })?;
        *color = parse_literal_color(value).ok_or_else(|| {
            TuiThemeError::new(
                TuiThemeErrorClass::Invalid,
                format!("{}: palette.{key}", path.to_string_lossy()),
            )
        })?;
    }
    Ok(UiTheme::from_base16(
        definition.name,
        Some(definition.author),
        Base16Palette::new(colors),
    ))
}

fn parse_palette(
    values: &BTreeMap<String, String>,
    path: &Path,
) -> Result<BTreeMap<String, Color>, TuiThemeError> {
    let mut palette = BTreeMap::new();
    for (name, value) in values {
        if !valid_theme_name(name) || name.len() > 64 {
            return Err(TuiThemeError::new(
                TuiThemeErrorClass::Invalid,
                format!("{}: palette.{name}", path.to_string_lossy()),
            ));
        }
        let color = parse_literal_color(value).ok_or_else(|| {
            TuiThemeError::new(
                TuiThemeErrorClass::Invalid,
                format!("{}: palette.{name}", path.to_string_lossy()),
            )
        })?;
        palette.insert(name.clone(), color);
    }
    Ok(palette)
}

fn apply_style(
    mut base: Style,
    definition: StyleDto,
    palette: &BTreeMap<String, Color>,
    path: &Path,
    role: &str,
) -> Result<Style, TuiThemeError> {
    if let Some(value) = definition.fg {
        base.fg = Some(parse_color(&value, palette, path, role)?);
    }
    if let Some(value) = definition.bg {
        base.bg = Some(parse_color(&value, palette, path, role)?);
    }
    if let Some(value) = definition.underline {
        base.underline_color = Some(parse_color(&value, palette, path, role)?);
    }
    if let Some(values) = definition.modifiers {
        if values.len() > 9 {
            return Err(style_error(path, role));
        }
        let mut modifiers = Modifier::empty();
        for value in values {
            modifiers |= parse_modifier(&value).ok_or_else(|| style_error(path, role))?;
        }
        base.add_modifier = modifiers;
        base.sub_modifier = Modifier::all() & !modifiers;
    }
    Ok(base)
}

fn parse_color(
    value: &str,
    palette: &BTreeMap<String, Color>,
    path: &Path,
    role: &str,
) -> Result<Color, TuiThemeError> {
    palette
        .get(value)
        .copied()
        .or_else(|| parse_literal_color(value))
        .ok_or_else(|| style_error(path, role))
}

fn parse_literal_color(value: &str) -> Option<Color> {
    match value {
        "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" => Some(Color::Gray),
        "dark-gray" => Some(Color::DarkGray),
        "light-red" => Some(Color::LightRed),
        "light-green" => Some(Color::LightGreen),
        "light-yellow" => Some(Color::LightYellow),
        "light-blue" => Some(Color::LightBlue),
        "light-magenta" => Some(Color::LightMagenta),
        "light-cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => parse_indexed(value).or_else(|| parse_rgb(value)),
    }
}

fn parse_indexed(value: &str) -> Option<Color> {
    value
        .strip_prefix("ansi:")?
        .parse::<u8>()
        .ok()
        .map(Color::Indexed)
}

fn parse_rgb(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

fn parse_modifier(value: &str) -> Option<Modifier> {
    match value {
        "bold" => Some(Modifier::BOLD),
        "dim" => Some(Modifier::DIM),
        "italic" => Some(Modifier::ITALIC),
        "underlined" => Some(Modifier::UNDERLINED),
        "slow-blink" => Some(Modifier::SLOW_BLINK),
        "rapid-blink" => Some(Modifier::RAPID_BLINK),
        "reversed" => Some(Modifier::REVERSED),
        "hidden" => Some(Modifier::HIDDEN),
        "crossed-out" => Some(Modifier::CROSSED_OUT),
        _ => None,
    }
}

fn discover_theme_files(directory: &Path) -> Result<Vec<PathBuf>, TuiThemeError> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Err(TuiThemeError::new(
                TuiThemeErrorClass::FileSystem,
                directory.to_string_lossy(),
            ));
        }
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(TuiThemeError::new(
                TuiThemeErrorClass::UnsafeFile,
                directory.to_string_lossy(),
            ));
        }
        Ok(_) => {}
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| {
        TuiThemeError::new(TuiThemeErrorClass::FileSystem, directory.to_string_lossy())
    })? {
        let entry = entry.map_err(|_| {
            TuiThemeError::new(TuiThemeErrorClass::FileSystem, directory.to_string_lossy())
        })?;
        let path = entry.path();
        if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("toml" | "yaml" | "yml")
        ) {
            paths.push(path);
            if paths.len() > MAX_THEME_FILES {
                return Err(TuiThemeError::new(
                    TuiThemeErrorClass::Malformed,
                    format!("{}: too many theme files", directory.to_string_lossy()),
                ));
            }
        }
    }
    paths.sort();
    Ok(paths)
}

fn read_theme_file(path: &Path) -> Result<Vec<u8>, TuiThemeError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        let class = if error.kind() == std::io::ErrorKind::NotFound {
            TuiThemeErrorClass::NotFound
        } else {
            TuiThemeErrorClass::FileSystem
        };
        TuiThemeError::new(class, path.to_string_lossy())
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || unsafe_permissions(&metadata) {
        return Err(TuiThemeError::new(
            TuiThemeErrorClass::UnsafeFile,
            path.to_string_lossy(),
        ));
    }
    if metadata.len() > MAX_THEME_BYTES {
        return Err(TuiThemeError::new(
            TuiThemeErrorClass::Malformed,
            format!(
                "{}: file exceeds {MAX_THEME_BYTES} bytes",
                path.to_string_lossy()
            ),
        ));
    }
    let file = File::open(path)
        .map_err(|_| TuiThemeError::new(TuiThemeErrorClass::FileSystem, path.to_string_lossy()))?;
    let mut bytes = Vec::new();
    file.take(MAX_THEME_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| TuiThemeError::new(TuiThemeErrorClass::FileSystem, path.to_string_lossy()))?;
    if bytes.len() as u64 > MAX_THEME_BYTES {
        return Err(TuiThemeError::new(
            TuiThemeErrorClass::Malformed,
            format!(
                "{}: file exceeds {MAX_THEME_BYTES} bytes",
                path.to_string_lossy()
            ),
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn unsafe_permissions(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o022 != 0
}

#[cfg(not(unix))]
const fn unsafe_permissions(_metadata: &fs::Metadata) -> bool {
    false
}

fn validate_metadata(value: &str, field: &str, path: &Path) -> Result<(), TuiThemeError> {
    if value.is_empty() || value.len() > MAX_METADATA_BYTES || value.chars().any(char::is_control) {
        Err(TuiThemeError::new(
            TuiThemeErrorClass::Invalid,
            format!("{}: {field}", path.to_string_lossy()),
        ))
    } else {
        Ok(())
    }
}

fn style_error(path: &Path, role: &str) -> TuiThemeError {
    TuiThemeError::new(
        TuiThemeErrorClass::Invalid,
        format!("{}: styles.{role}", path.to_string_lossy()),
    )
}

fn builtin_theme(name: &str) -> Result<Option<UiTheme>, TuiThemeError> {
    match name {
        "terminal" => Ok(Some(UiTheme::terminal())),
        "no-color" => Ok(Some(UiTheme::no_color())),
        _ => BUILTIN_BASE16
            .iter()
            .find(|(selector, _)| *selector == name)
            .map(|(_, bytes)| parse_base16(bytes, Path::new(name)))
            .transpose(),
    }
}

fn is_builtin_name(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

fn valid_theme_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn file_stem(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    valid_theme_name(stem).then(|| stem.to_owned())
}

fn theme_source(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("toml") => "toml",
        Some("yaml" | "yml") => "base16",
        _ => "file",
    }
}

fn bounded_subject(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_METADATA_BYTES)
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use hq_tui::UiThemeRole;
    use ratatui::style::{Color, Modifier};

    use super::{
        BUILTIN_NAMES, NativeThemeDto, StyleDto, ThemeResolver, TuiThemeEnvironment, builtin_theme,
        parse_base16, parse_literal_color,
    };

    #[test]
    fn color_vocabulary_is_exact() {
        assert_eq!(parse_literal_color("reset"), Some(Color::Reset));
        assert_eq!(parse_literal_color("ansi:255"), Some(Color::Indexed(255)));
        assert_eq!(
            parse_literal_color("#d79921"),
            Some(Color::Rgb(215, 153, 33))
        );
        assert_eq!(parse_literal_color("#abcd"), None);
        assert_eq!(parse_literal_color("ansi:256"), None);
        assert_eq!(parse_literal_color("transparent"), None);
    }

    #[test]
    fn native_override_replaces_colors_and_modifier_set() {
        let resolver = ThemeResolver::new(None);
        let definition = NativeThemeDto {
            name: Some("Example".to_owned()),
            author: Some("A. User".to_owned()),
            inherits: Some("terminal".to_owned()),
            palette: BTreeMap::from([
                ("paper".to_owned(), "#282828".to_owned()),
                ("ink".to_owned(), "#ebdbb2".to_owned()),
            ]),
            styles: BTreeMap::from([(
                "ui.screen".to_owned(),
                StyleDto {
                    fg: Some("ink".to_owned()),
                    bg: Some("paper".to_owned()),
                    underline: None,
                    modifiers: Some(vec!["bold".to_owned()]),
                },
            )]),
        };
        let theme = resolver
            .resolve_native(
                definition,
                Path::new("example.toml"),
                "example",
                0,
                &mut Vec::new(),
            )
            .expect("valid native theme");
        let screen = theme.style(UiThemeRole::Screen);
        assert_eq!(screen.fg, Some(Color::Rgb(235, 219, 178)));
        assert_eq!(screen.bg, Some(Color::Rgb(40, 40, 40)));
        assert!(screen.add_modifier.contains(Modifier::BOLD));
        assert!(screen.sub_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn base16_mapping_requires_the_complete_current_schema() {
        let yaml = br##"
system: "base16"
name: "Example Dark"
author: "Theme Author"
variant: "dark"
palette:
  base00: "#000000"
  base01: "#111111"
  base02: "#222222"
  base03: "#333333"
  base04: "#444444"
  base05: "#555555"
  base06: "#666666"
  base07: "#777777"
  base08: "#880000"
  base09: "#990000"
  base0A: "#aaaa00"
  base0B: "#00bb00"
  base0C: "#00cccc"
  base0D: "#0000dd"
  base0E: "#ee00ee"
  base0F: "#ff00ff"
"##;
        let theme = parse_base16(yaml, Path::new("example.yaml")).expect("valid Base16 theme");
        assert_eq!(theme.name(), "Example Dark");
        assert_eq!(theme.author(), Some("Theme Author"));
        assert_eq!(
            theme.style(UiThemeRole::Screen).bg,
            Some(Color::Rgb(0, 0, 0))
        );
        assert_eq!(
            theme.style(UiThemeRole::Text).fg,
            Some(Color::Rgb(85, 85, 85))
        );
        assert!(parse_base16(&yaml[..yaml.len() - 20], Path::new("broken.yaml")).is_err());
    }

    #[test]
    fn automatic_default_is_gruvbox_dark_medium() {
        assert_eq!(
            super::resolve_tui_theme(None, &TuiThemeEnvironment::default())
                .expect("automatic Gruvbox theme")
                .name(),
            "Gruvbox dark, medium"
        );
    }

    #[test]
    fn no_color_only_applies_without_an_explicit_selection() {
        let environment = TuiThemeEnvironment {
            no_color: Some("1".into()),
            ..TuiThemeEnvironment::default()
        };
        assert_eq!(
            super::resolve_tui_theme(None, &environment)
                .expect("automatic no-color")
                .name(),
            "no-color"
        );
        let explicit = crate::ThemeSelection::new("terminal".to_owned()).expect("selection");
        assert_eq!(
            super::resolve_tui_theme(Some(&explicit), &environment)
                .expect("explicit theme wins")
                .name(),
            "terminal"
        );
    }

    #[test]
    fn bundled_themes_resolve_through_the_base16_importer() {
        for selector in BUILTIN_NAMES {
            let theme = builtin_theme(selector)
                .expect("bundled theme parses")
                .expect("bundled selector exists");
            assert!(!theme.name().is_empty());
            if selector.starts_with("gruvbox-") {
                assert!(theme.author().is_some());
                assert!(theme.style(UiThemeRole::Screen).bg.is_some());
                assert!(theme.style(UiThemeRole::Text).fg.is_some());
            }
        }
        for (selector, expected_background) in [
            ("gruvbox-dark-hard", Color::Rgb(29, 32, 33)),
            ("gruvbox-dark-medium", Color::Rgb(40, 40, 40)),
            ("gruvbox-dark-soft", Color::Rgb(50, 48, 47)),
            ("gruvbox-light-hard", Color::Rgb(249, 245, 215)),
            ("gruvbox-light-medium", Color::Rgb(251, 241, 199)),
            ("gruvbox-light-soft", Color::Rgb(242, 229, 188)),
        ] {
            let theme = builtin_theme(selector)
                .expect("bundled theme parses")
                .expect("bundled selector exists");
            assert_eq!(
                theme.style(UiThemeRole::Screen).bg,
                Some(expected_background)
            );
        }
    }
}
