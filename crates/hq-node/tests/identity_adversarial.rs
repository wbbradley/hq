//! Adversarial filesystem, redaction, backup, and configuration contracts.

#![allow(clippy::expect_used)]

use std::{fs, path::PathBuf};

use hq_node::{
    BackupPassword, IdentityErrorClass, LocalCodexConfiguration, StateDirectoryOwner, StatePaths,
    ThemeSelection,
};

mod support;

use support::{TestDirectory, write_private};

#[test]
fn malformed_truncated_trailing_and_invalid_identity_files_fail_closed() {
    for bytes in [Vec::new(), b"HQIDV1\0\0\x01".to_vec(), vec![0_u8; 73], {
        let mut trailing = vec![0_u8; 74];
        trailing[..8].copy_from_slice(b"HQIDV1\0\0");
        trailing[8] = 1;
        trailing[9] = 1;
        trailing
    }] {
        let directory = TestDirectory::new();
        let paths = StatePaths::new(directory.path().join("state")).expect("path is valid");
        let owner = StateDirectoryOwner::acquire(paths.clone()).expect("owner acquires");
        write_private(paths.identity_file(), &bytes);
        assert_eq!(
            owner
                .load_identity()
                .expect_err("malformed identity is rejected")
                .class(),
            IdentityErrorClass::IdentityMalformed
        );
    }
}

#[test]
fn abandoned_temporary_files_do_not_become_identity_and_collisions_are_cleaned() {
    let directory = TestDirectory::new();
    let paths = StatePaths::new(directory.path().join("state")).expect("path is valid");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("owner acquires");
    write_private(&paths.root().join("identity.v1.tmp-abandoned"), b"partial");
    let identity = owner
        .initialize()
        .expect("an unrelated partial file does not block initialization");

    let password = BackupPassword::new("backup password".to_owned()).expect("password is valid");
    let backup = directory.path().join("backup.json");
    owner
        .export_identity(&identity, &password, &backup)
        .expect("first export succeeds");
    assert_eq!(
        owner
            .export_identity(&identity, &password, &backup)
            .expect_err("collision fails")
            .class(),
        IdentityErrorClass::BackupExists
    );
    let leaked_temporary = fs::read_dir(directory.path())
        .expect("directory reads")
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("backup.json.tmp-")
        });
    assert!(!leaked_temporary);

    let mut package = fs::read(&backup).expect("backup reads");
    assert!(!String::from_utf8_lossy(&package).contains("backup password"));
    let target = TestDirectory::new();
    let target_owner = StateDirectoryOwner::acquire(
        StatePaths::new(target.path().join("state")).expect("target path is valid"),
    )
    .expect("target owner acquires");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o644))
            .expect("backup mode broadens");
        assert_eq!(
            target_owner
                .import_identity(&backup, &password)
                .expect_err("unsafe backup mode is rejected")
                .class(),
            IdentityErrorClass::UnsafePermissions
        );
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))
            .expect("backup mode restores");
    }
    let start = package
        .windows(b"ncryptsec1".len())
        .position(|window| window == b"ncryptsec1")
        .expect("encrypted key is present");
    let end = package[start..]
        .iter()
        .position(|byte| *byte == b'"')
        .map(|offset| start + offset)
        .expect("encrypted key string ends");
    package[end - 1] = if package[end - 1] == b'q' { b'p' } else { b'q' };
    write_private(&backup, &package);
    assert_eq!(
        target_owner
            .import_identity(&backup, &password)
            .expect_err("corrupted checksum is rejected before decryption")
            .class(),
        IdentityErrorClass::BackupMalformed
    );
    write_private(&backup, &vec![b'x'; 4_097]);
    assert_eq!(
        target_owner
            .import_identity(&backup, &password)
            .expect_err("oversized backup is rejected before decoding")
            .class(),
        IdentityErrorClass::BackupMalformed
    );
}

#[test]
fn configuration_rejects_noncanonical_duplicates_invalid_values_and_unsafe_modes() {
    let directory = TestDirectory::new();
    let paths = StatePaths::new(directory.path().join("state")).expect("path is valid");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("owner acquires");

    for malformed in [
        br#"{"default_provider":null,"version":1}"#.as_slice(),
        br#"{"version":2,"default_provider":null}"#.as_slice(),
        br#"{"version":1,"unknown":null,"default_provider":null}"#.as_slice(),
        br#"{"version":1,"default_provider":null,"theme":null}"#.as_slice(),
    ] {
        write_private(paths.configuration_file(), malformed);
        assert_eq!(
            owner
                .load_configuration()
                .expect_err("noncanonical config is rejected")
                .class(),
            IdentityErrorClass::ConfigurationMalformed
        );
    }

    assert!(LocalCodexConfiguration::new(false, Some(String::new())).is_err());
    assert!(ThemeSelection::new("gruvbox-dark-hard".to_owned()).is_ok());
    assert!(ThemeSelection::new("/tmp/hq-theme.toml".to_owned()).is_ok());
    assert!(ThemeSelection::new("/tmp/../hq-theme.toml".to_owned()).is_err());
    assert!(ThemeSelection::new("relative/theme.toml".to_owned()).is_err());
    assert!(ThemeSelection::new("x".repeat(1_025)).is_err());
    write_private(paths.configuration_file(), &vec![b'x'; 65_537]);
    assert_eq!(
        owner
            .load_configuration()
            .expect_err("configuration file is bounded before decoding")
            .class(),
        IdentityErrorClass::ConfigurationMalformed
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            paths.configuration_file(),
            fs::Permissions::from_mode(0o644),
        )
        .expect("mode changes");
        assert_eq!(
            owner
                .load_configuration()
                .expect_err("broad mode is rejected")
                .class(),
            IdentityErrorClass::UnsafePermissions
        );
    }
}

#[test]
fn password_and_error_surfaces_are_redacted_and_bounded() {
    let password = BackupPassword::new("never print me".to_owned()).expect("password is valid");
    assert_eq!(format!("{password:?}"), "BackupPassword([REDACTED])");
    assert_eq!(
        BackupPassword::new(String::new())
            .expect_err("empty password fails")
            .class(),
        IdentityErrorClass::PasswordInvalid
    );
    assert_eq!(
        BackupPassword::new("x".repeat(1_025))
            .expect_err("oversized password fails")
            .class(),
        IdentityErrorClass::PasswordInvalid
    );
    let rendered = IdentityErrorClass::FileSystem;
    let directory = TestDirectory::new();
    let relative = StatePaths::new(PathBuf::from("relative")).expect_err("relative path rejected");
    assert_eq!(relative.class(), IdentityErrorClass::InvalidPath);
    assert!(
        !relative
            .to_string()
            .contains(&directory.path().display().to_string())
    );
    assert_eq!(rendered, IdentityErrorClass::FileSystem);
}

#[cfg(unix)]
#[test]
fn state_artifact_symlinks_and_broad_directory_permissions_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let broad = TestDirectory::new();
    let broad_root = broad.path().join("state");
    fs::create_dir(&broad_root).expect("state root creates");
    fs::set_permissions(&broad_root, fs::Permissions::from_mode(0o755)).expect("mode changes");
    assert_eq!(
        StateDirectoryOwner::acquire(StatePaths::new(broad_root).expect("path valid"))
            .expect_err("broad root rejected")
            .class(),
        IdentityErrorClass::UnsafePermissions
    );

    let linked = TestDirectory::new();
    let real = linked.path().join("real");
    fs::create_dir(&real).expect("real root creates");
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).expect("real mode changes");
    let alias = linked.path().join("alias");
    symlink(&real, &alias).expect("root symlink creates");
    assert_eq!(
        StateDirectoryOwner::acquire(StatePaths::new(alias).expect("alias path valid"))
            .expect_err("root symlink rejected")
            .class(),
        IdentityErrorClass::SymbolicLink
    );

    let identity_link = TestDirectory::new();
    let paths = StatePaths::new(identity_link.path().join("state")).expect("path valid");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("owner acquires");
    let elsewhere = identity_link.path().join("elsewhere");
    write_private(&elsewhere, b"not an identity");
    symlink(&elsewhere, paths.identity_file()).expect("identity symlink creates");
    assert_eq!(
        owner
            .initialize()
            .expect_err("identity symlink rejected")
            .class(),
        IdentityErrorClass::SymbolicLink
    );

    let config_link = TestDirectory::new();
    let paths = StatePaths::new(config_link.path().join("state")).expect("path valid");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("owner acquires");
    let elsewhere = config_link.path().join("elsewhere-config");
    write_private(&elsewhere, b"{}");
    symlink(&elsewhere, paths.configuration_file()).expect("config symlink creates");
    assert_eq!(
        owner
            .load_configuration()
            .expect_err("configuration symlink rejected")
            .class(),
        IdentityErrorClass::SymbolicLink
    );
}
