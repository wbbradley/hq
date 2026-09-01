//! Public contracts for secure installation identity and local configuration persistence.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use hq_domain::ProviderId;
use hq_node::{
    BackupPassword, IdentityErrorClass, LocalCodexConfiguration, LocalConfiguration, RelayEndpoint,
    StateDirectoryOwner, StatePaths, ThemeSelection,
};

mod support;

use support::{TestDirectory, assert_private_mode};

#[test]
fn state_paths_are_explicit_and_environment_derivation_is_deterministic() {
    let paths = StatePaths::new(PathBuf::from("/var/lib/example-hq"))
        .expect("absolute explicit root is valid");
    assert_eq!(paths.root(), Path::new("/var/lib/example-hq"));
    assert_eq!(
        paths.identity_file(),
        Path::new("/var/lib/example-hq/identity.v1")
    );
    assert_eq!(
        paths.configuration_file(),
        Path::new("/var/lib/example-hq/local-config.v1.json")
    );
    assert_eq!(
        paths.database_file(),
        Path::new("/var/lib/example-hq/hq.sqlite3")
    );
    assert_eq!(
        paths.ownership_file(),
        Path::new("/var/lib/example-hq/node.lock")
    );

    let xdg = StatePaths::derive(Some(Path::new("/state")), Some(Path::new("/home/alice")))
        .expect("XDG state root wins");
    assert_eq!(xdg.root(), Path::new("/state/hq"));
    let fallback =
        StatePaths::derive(None, Some(Path::new("/home/alice"))).expect("home fallback is valid");
    assert_eq!(fallback.root(), Path::new("/home/alice/.local/state/hq"));
    assert_eq!(
        StatePaths::derive(None, None)
            .expect_err("missing path inputs fail")
            .class(),
        IdentityErrorClass::PathUnavailable
    );
}

#[test]
fn initialization_is_atomic_stable_redacted_and_exclusively_owned() {
    let directory = TestDirectory::new();
    let paths = StatePaths::new(directory.path().join("state")).expect("test path is valid");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("first owner acquires lock");
    assert_eq!(
        StateDirectoryOwner::acquire(paths.clone())
            .expect_err("second same-state owner is rejected")
            .class(),
        IdentityErrorClass::AlreadyOwned
    );

    let identity = owner.initialize().expect("fresh identity initializes");
    let public = identity.public_identity();
    assert_eq!(identity.signer().public_key(), public.signing_public_key);
    assert_eq!(public.installation_id.as_bytes().len(), 32);
    assert!(!public.fingerprint.is_empty());
    let debug = format!("{identity:?}");
    assert!(debug.contains(&public.fingerprint));
    assert!(!debug.contains("secret"));

    let reopened = owner.load_identity().expect("identity reopens");
    assert_eq!(reopened.public_identity(), public);
    assert_eq!(
        owner
            .initialize()
            .expect_err("initialization never overwrites")
            .class(),
        IdentityErrorClass::IdentityExists
    );

    assert_private_mode(paths.root(), 0o700);
    assert_private_mode(paths.identity_file(), 0o600);
    assert_private_mode(paths.ownership_file(), 0o600);
}

#[test]
fn encrypted_backup_round_trip_preserves_authority_and_refuses_overwrite() {
    let source_directory = TestDirectory::new();
    let source_paths =
        StatePaths::new(source_directory.path().join("source")).expect("source path is valid");
    let source = StateDirectoryOwner::acquire(source_paths).expect("source owner acquires");
    let identity = source.initialize().expect("source identity initializes");
    let expected = identity.public_identity();
    let password = BackupPassword::new("correct horse battery staple".to_owned())
        .expect("password is bounded");
    let backup = source_directory.path().join("identity-backup.json");
    source
        .export_identity(&identity, &password, &backup)
        .expect("backup exports");
    assert_private_mode(&backup, 0o600);
    assert_eq!(
        source
            .export_identity(&identity, &password, &backup)
            .expect_err("backup export never overwrites")
            .class(),
        IdentityErrorClass::BackupExists
    );

    let target_directory = TestDirectory::new();
    let target_paths =
        StatePaths::new(target_directory.path().join("target")).expect("target path is valid");
    let target = StateDirectoryOwner::acquire(target_paths).expect("target owner acquires");
    let imported = target
        .import_identity(&backup, &password)
        .expect("backup imports");
    assert_eq!(imported.public_identity(), expected);
    assert_eq!(
        target
            .import_identity(&backup, &password)
            .expect_err("import never overwrites identity")
            .class(),
        IdentityErrorClass::IdentityExists
    );

    let wrong_target = TestDirectory::new();
    let wrong_owner = StateDirectoryOwner::acquire(
        StatePaths::new(wrong_target.path().join("target")).expect("wrong target path is valid"),
    )
    .expect("wrong target owner acquires");
    let wrong_password = BackupPassword::new("wrong password".to_owned()).expect("password valid");
    assert_eq!(
        wrong_owner
            .import_identity(&backup, &wrong_password)
            .expect_err("wrong password fails closed")
            .class(),
        IdentityErrorClass::BackupAuthenticationFailed
    );
}

#[test]
fn unsigned_local_configuration_is_typed_canonical_and_atomically_replaceable() {
    let directory = TestDirectory::new();
    let paths = StatePaths::new(directory.path().join("state")).expect("test path is valid");
    let owner = StateDirectoryOwner::acquire(paths.clone()).expect("owner acquires");
    assert_eq!(
        owner.load_configuration().expect("missing config defaults"),
        LocalConfiguration::default()
    );

    let configuration = LocalConfiguration::new(
        [
            RelayEndpoint::new("wss://relay.example".to_owned()).expect("relay is valid"),
            RelayEndpoint::new("ws://127.0.0.1:8080".to_owned()).expect("local relay is valid"),
        ],
        Some(ProviderId::new("codex").expect("provider is valid")),
    )
    .expect("configuration is valid");
    owner
        .store_configuration(&configuration)
        .expect("configuration stores");
    assert_eq!(
        owner.load_configuration().expect("configuration loads"),
        configuration
    );
    assert_private_mode(paths.configuration_file(), 0o600);
    assert_eq!(
        fs::read(paths.configuration_file()).expect("configuration bytes"),
        br#"{"version":1,"relays":["ws://127.0.0.1:8080","wss://relay.example"],"default_provider":"codex"}"#,
        "an unset theme keeps legacy version-1 bytes unchanged"
    );

    let themed = LocalConfiguration::from_parts(
        configuration.relays.clone(),
        configuration.default_provider.clone(),
        Some(ThemeSelection::new("gruvbox-dark-hard".to_owned()).expect("theme selector")),
        configuration.codex,
    )
    .expect("themed configuration is valid");
    owner
        .store_configuration(&themed)
        .expect("themed configuration stores");
    assert_eq!(paths.load_configuration().expect("read-only load"), themed);
    assert!(
        String::from_utf8(fs::read(paths.configuration_file()).expect("configuration bytes"))
            .expect("configuration is UTF-8")
            .ends_with("\"theme\":\"gruvbox-dark-hard\"}"),
        "theme selection is persisted canonically"
    );

    let yolo = LocalConfiguration::from_parts(
        themed.relays.clone(),
        themed.default_provider.clone(),
        themed.theme.clone(),
        LocalCodexConfiguration { yolo: true },
    )
    .expect("Codex configuration is valid");
    owner
        .store_configuration(&yolo)
        .expect("Codex configuration stores");
    assert_eq!(paths.load_configuration().expect("read-only load"), yolo);
    assert!(
        String::from_utf8(fs::read(paths.configuration_file()).expect("configuration bytes"))
            .expect("configuration is UTF-8")
            .ends_with("\"codex\":{\"yolo\":true}}"),
        "Codex YOLO is persisted canonically"
    );

    let replacement = LocalConfiguration::new(
        [RelayEndpoint::new("wss://other.example".to_owned()).expect("relay is valid")],
        None,
    )
    .expect("replacement is valid");
    owner
        .store_configuration(&replacement)
        .expect("configuration replaces atomically");
    assert_eq!(
        owner.load_configuration().expect("replacement loads"),
        replacement
    );

    let duplicate =
        RelayEndpoint::new("wss://duplicate.example".to_owned()).expect("relay is valid");
    let invalid = LocalConfiguration {
        relays: vec![duplicate.clone(), duplicate],
        default_provider: None,
        theme: None,
        codex: LocalCodexConfiguration::default(),
    };
    assert_eq!(
        owner
            .store_configuration(&invalid)
            .expect_err("public fields are revalidated before persistence")
            .class(),
        IdentityErrorClass::ConfigurationInvalid
    );
    assert_eq!(
        owner
            .load_configuration()
            .expect("invalid replacement leaves prior value intact"),
        replacement
    );
}
