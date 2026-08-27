//! The single user-facing HQ executable.

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    use std::io::Write as _;

    let result = hq_node::parse_node_cli(std::env::args_os().skip(1))
        .and_then(|command| hq_node::run_node_cli(&command));
    match result {
        Ok(output) if std::io::stdout().write_all(output.as_bytes()).is_ok() => {}
        Ok(_) => std::process::exit(1),
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "hq: {error}");
            std::process::exit(1);
        }
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
