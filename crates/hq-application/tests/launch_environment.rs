//! Secret-bearing launch-context boundary contracts.

#![allow(clippy::expect_used)]

use hq_application::{ApplicationErrorCode, LaunchEnvironment};

#[test]
fn launch_environment_copies_sorts_and_redacts_secret_values() {
    let source = [
        ("TOKEN", b"super-secret".as_slice()),
        ("PATH", b"/usr/bin".as_slice()),
    ];
    let environment = LaunchEnvironment::copy_from(source).expect("valid launch environment");
    let mut observed = Vec::new();
    environment.visit(|name, value| observed.push((name.to_owned(), value.to_vec())));
    assert_eq!(
        observed,
        [
            ("PATH".to_owned(), b"/usr/bin".to_vec()),
            ("TOKEN".to_owned(), b"super-secret".to_vec()),
        ]
    );

    let diagnostic = format!("{environment:?}");
    assert_eq!(diagnostic, "LaunchEnvironment { entry_count: 2, .. }");
    assert!(!diagnostic.contains("TOKEN"));
    assert!(!diagnostic.contains("super-secret"));
}

#[test]
fn launch_environment_rejects_ambiguous_or_process_invalid_entries() {
    let duplicate =
        LaunchEnvironment::copy_from([("TOKEN", b"one".as_slice()), ("TOKEN", b"two".as_slice())]);
    assert!(matches!(
        duplicate,
        Err(error) if error.code() == ApplicationErrorCode::InvalidRequest
    ));
    let nul = LaunchEnvironment::copy_from([("TOKEN", b"bad\0value".as_slice())]);
    assert!(matches!(
        nul,
        Err(error) if error.code() == ApplicationErrorCode::InvalidRequest
    ));
}
