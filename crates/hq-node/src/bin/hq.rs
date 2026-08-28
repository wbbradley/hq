//! The single user-facing HQ executable.

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    use std::io::Write as _;

    let execution = hq_node::execute_cli(std::env::args_os().skip(1));
    if std::io::stdout()
        .write_all(execution.stdout.as_bytes())
        .is_err()
        || std::io::stderr()
            .write_all(execution.stderr.as_bytes())
            .is_err()
    {
        std::process::exit(1);
    }
    if execution.exit_code != 0 {
        std::process::exit(i32::from(execution.exit_code));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    use std::io::Write as _;

    let _ = writeln!(
        std::io::stderr(),
        "hq: local node transport is unsupported on this platform"
    );
    std::process::exit(1);
}
