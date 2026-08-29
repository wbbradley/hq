//! The single user-facing HQ executable.

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    use std::io::{IsTerminal as _, Write as _};

    let mut arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    if !has_top_level_command(&arguments) && (!stdin.is_terminal() || !stdout.is_terminal()) {
        arguments.push("list".into());
    }
    let execution = if stdin.is_terminal()
        && arguments
            .iter()
            .any(|argument| argument == "--password-stdin")
    {
        hq_node::execute_cli(arguments)
    } else {
        hq_node::execute_cli_with_input(arguments, &mut stdin.lock())
    };
    let mut stdout = stdout.lock();
    if stdout.write_all(execution.stdout.as_bytes()).is_err() || stdout.flush().is_err() {
        std::process::exit(1);
    }
    drop(stdout);
    if let Some(completion) = &execution.completion
        && hq_node::complete_cli_delivery(completion).is_err()
    {
        let _ = std::io::stderr().write_all(
            b"hq: delivered output but could not complete receipt; retry may repeat stable message IDs\n",
        );
        std::process::exit(1);
    }
    if std::io::stderr()
        .write_all(execution.stderr.as_bytes())
        .is_err()
    {
        std::process::exit(1);
    }
    if execution.exit_code != 0 {
        std::process::exit(i32::from(execution.exit_code));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn has_top_level_command(arguments: &[std::ffi::OsString]) -> bool {
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].to_str() {
            Some("--output" | "--state-root") if index + 1 < arguments.len() => index += 2,
            _ => return true,
        }
    }
    false
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
