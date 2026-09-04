//! Theme discovery, safety, inheritance, and installed-startup contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{fs, path::Path};

use hq_node::{
    LocalConfiguration, StateDirectoryOwner, StatePaths, ThemeSelection, TuiShellError,
    TuiThemeEnvironment, TuiThemeErrorClass, list_tui_themes, resolve_installed_tui_theme,
    resolve_tui_theme,
};
use hq_tui::UiThemeRole;
use ratatui::style::Color;

mod support;

use support::TestDirectory;

fn environment(directory: &TestDirectory) -> TuiThemeEnvironment {
    TuiThemeEnvironment {
        xdg_config_home: Some(directory.path().join("config")),
        home_directory: None,
        no_color: None,
    }
}

fn theme_directory(environment: &TuiThemeEnvironment) -> std::path::PathBuf {
    environment.theme_directory().expect("theme directory")
}

fn write_theme(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("theme parent")).expect("create theme directory");
    fs::write(path, text).expect("write theme");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private theme mode");
    }
}

#[test]
fn native_themes_inherit_and_override_exact_semantic_roles() {
    let directory = TestDirectory::new();
    let environment = environment(&directory);
    let themes = theme_directory(&environment);
    write_theme(
        &themes.join("foundation.toml"),
        r##"
name = "Foundation"
inherits = "terminal"

[palette]
paper = "#111111"
ink = "#eeeeee"

[styles."ui.screen"]
fg = "ink"
bg = "paper"

[styles."status.error"]
fg = "ansi:160"
"##,
    );
    write_theme(
        &themes.join("personal.toml"),
        r##"
name = "Personal"
inherits = "foundation"

[styles."ui.screen"]
bg = "#222222"

[styles."ui.selection.focused"]
fg = "white"
bg = "blue"
modifiers = ["bold", "underlined"]
"##,
    );

    let selection = ThemeSelection::new("personal".to_owned()).expect("selection");
    let theme = resolve_tui_theme(Some(&selection), &environment).expect("theme resolves");
    assert_eq!(theme.name(), "Personal");
    assert_eq!(
        theme.style(UiThemeRole::Screen).fg,
        Some(Color::Rgb(238, 238, 238))
    );
    assert_eq!(
        theme.style(UiThemeRole::Screen).bg,
        Some(Color::Rgb(34, 34, 34))
    );
    assert_eq!(
        theme.style(UiThemeRole::Error).fg,
        Some(Color::Indexed(160))
    );
}

#[test]
fn malformed_cycles_depth_duplicates_and_oversized_files_fail_closed() {
    let directory = TestDirectory::new();
    let environment = environment(&directory);
    let themes = theme_directory(&environment);

    write_theme(&themes.join("cycle-a.toml"), "inherits = \"cycle-b\"\n");
    write_theme(&themes.join("cycle-b.toml"), "inherits = \"cycle-a\"\n");
    let cycle = ThemeSelection::new("cycle-a".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&cycle), &environment)
            .expect_err("cycle rejected")
            .class(),
        TuiThemeErrorClass::Inheritance
    );

    for depth in 0..=9 {
        let parent = if depth == 9 {
            "terminal".to_owned()
        } else {
            format!("depth-{}", depth + 1)
        };
        write_theme(
            &themes.join(format!("depth-{depth}.toml")),
            &format!("inherits = \"{parent}\"\n"),
        );
    }
    let deep = ThemeSelection::new("depth-0".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&deep), &environment)
            .expect_err("depth rejected")
            .class(),
        TuiThemeErrorClass::Inheritance
    );

    write_theme(&themes.join("duplicate.toml"), "inherits = \"terminal\"\n");
    write_theme(
        &themes.join("duplicate.yaml"),
        "system: invalid\nname: duplicate\nauthor: test\nvariant: dark\npalette: {}\n",
    );
    let duplicate = ThemeSelection::new("duplicate".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&duplicate), &environment)
            .expect_err("duplicate rejected")
            .class(),
        TuiThemeErrorClass::Ambiguous
    );

    write_theme(
        &themes.join("unknown.toml"),
        "inherits = \"terminal\"\nunknown = true\n",
    );
    let unknown = ThemeSelection::new("unknown".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&unknown), &environment)
            .expect_err("unknown field rejected")
            .class(),
        TuiThemeErrorClass::Malformed
    );

    write_theme(
        &themes.join("bad-modifier.toml"),
        "inherits = \"terminal\"\n[styles.\"ui.text\"]\nmodifiers = [\"sparkle\"]\n",
    );
    let bad_modifier = ThemeSelection::new("bad-modifier".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&bad_modifier), &environment)
            .expect_err("unknown modifier rejected")
            .class(),
        TuiThemeErrorClass::Invalid
    );

    write_theme(&themes.join("oversized.toml"), &"x".repeat(65_537));
    let oversized = ThemeSelection::new("oversized".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&oversized), &environment)
            .expect_err("oversized theme rejected")
            .class(),
        TuiThemeErrorClass::Malformed
    );
}

#[cfg(unix)]
#[test]
fn symlinks_and_broadly_writable_theme_files_are_rejected() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let directory = TestDirectory::new();
    let environment = environment(&directory);
    let themes = theme_directory(&environment);
    let target = directory.path().join("target.toml");
    write_theme(&target, "inherits = \"terminal\"\n");
    fs::create_dir_all(&themes).expect("theme directory");
    symlink(&target, themes.join("linked.toml")).expect("theme symlink");
    let linked = ThemeSelection::new("linked".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&linked), &environment)
            .expect_err("symlink rejected")
            .class(),
        TuiThemeErrorClass::UnsafeFile
    );

    let writable = themes.join("writable.toml");
    write_theme(&writable, "inherits = \"terminal\"\n");
    fs::set_permissions(&writable, fs::Permissions::from_mode(0o622)).expect("writable mode");
    let writable = ThemeSelection::new("writable".to_owned()).expect("selection");
    assert_eq!(
        resolve_tui_theme(Some(&writable), &environment)
            .expect_err("broad permissions rejected")
            .class(),
        TuiThemeErrorClass::UnsafeFile
    );
}

#[test]
fn catalog_exposes_builtins_active_choice_and_invalid_user_files() {
    let directory = TestDirectory::new();
    let environment = environment(&directory);
    let themes = theme_directory(&environment);
    write_theme(
        &themes.join("broken.toml"),
        "inherits = \"terminal\"\n[styles.\"ui.screen\"]\nfg = \"missing\"\n",
    );
    let selected = ThemeSelection::new("gruvbox-light-soft".to_owned()).expect("selection");
    let catalog = list_tui_themes(Some(&selected), &environment).expect("catalog");
    assert_eq!(catalog.len(), 9);
    assert!(
        catalog
            .iter()
            .any(|entry| entry.selector == "gruvbox-light-soft" && entry.active)
    );
    assert!(
        catalog
            .iter()
            .any(|entry| entry.selector == "broken" && entry.error.is_some())
    );
}

#[test]
fn automatic_catalog_choice_is_gruvbox_dark_medium_unless_no_color_is_requested() {
    let directory = TestDirectory::new();
    let environment = environment(&directory);
    let catalog = list_tui_themes(None, &environment).expect("automatic catalog");
    assert!(
        catalog
            .iter()
            .any(|entry| { entry.selector == "gruvbox-dark-medium" && entry.active })
    );
    assert!(
        !catalog
            .iter()
            .any(|entry| entry.selector == "terminal" && entry.active)
    );

    let no_color = TuiThemeEnvironment {
        no_color: Some("1".into()),
        ..environment
    };
    let catalog = list_tui_themes(None, &no_color).expect("no-color catalog");
    assert!(
        catalog
            .iter()
            .any(|entry| entry.selector == "no-color" && entry.active)
    );
}

#[test]
fn installed_resolution_reports_selected_file_before_terminal_composition() {
    let directory = TestDirectory::new();
    let environment = environment(&directory);
    let paths = StatePaths::new(directory.path().join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("state owner");
    let missing = ThemeSelection::new("missing-theme".to_owned()).expect("selection");
    let configuration = LocalConfiguration::from_parts(
        None,
        Some(missing),
        hq_node::LocalCodexConfiguration::default(),
    )
    .expect("configuration is valid");
    owner
        .store_configuration(&configuration)
        .expect("configuration stores");

    let error = resolve_installed_tui_theme(&paths, &environment)
        .expect_err("missing selected theme fails before terminal construction");
    assert!(matches!(error, TuiShellError::Theme(_)));
    let (code, message) = error.diagnostic();
    assert_eq!(code, "tui.theme_invalid");
    assert!(message.contains("missing-theme"));
    assert!(message.contains("hq config themes"));
}
