//! The single user-facing HQ executable.

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    use std::io::{IsTerminal as _, Write as _};

    let mut arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let interactive = stdin.is_terminal() && stdout.is_terminal();
    if !has_top_level_command(&arguments) {
        arguments.push(if interactive { "tui" } else { "list" }.into());
    }
    let invocation = hq_node::parse_cli(arguments.clone()).ok();
    if matches!(
        invocation.as_ref().map(|invocation| &invocation.command),
        Some(hq_node::CliCommand::Daemon {
            action: hq_node::DaemonCommand::Run,
            ..
        })
    ) && close_inherited_descriptors().is_err()
    {
        let _ = std::io::stderr()
            .write_all(b"hq: daemon.isolation_failed: inherited descriptors could not be closed\n");
        std::process::exit(1);
    }
    if interactive
        && let Some(hq_node::CliInvocation {
            command: hq_node::CliCommand::Tui { state },
            ..
        }) = invocation
    {
        let result = hq_node::run_installed_tui(state);
        if let Err(error) = result {
            let (code, action) = error.diagnostic();
            let _ = writeln!(std::io::stderr(), "hq: {code}: {action}");
            std::process::exit(1);
        }
        return;
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

#[cfg(target_os = "linux")]
const PROCESS_DESCRIPTOR_DIRECTORY: &str = "/proc/self/fd";

#[cfg(target_os = "macos")]
const PROCESS_DESCRIPTOR_DIRECTORY: &str = "/dev/fd";

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn close_inherited_descriptors() -> Result<(), ()> {
    let descriptors = std::fs::read_dir(PROCESS_DESCRIPTOR_DIRECTORY)
        .map_err(|_| ())?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|descriptor| *descriptor > 2)
        .collect::<Vec<_>>();
    for descriptor in descriptors {
        let _ = nix::unistd::close(descriptor);
    }
    Ok(())
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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::has_top_level_command;
    use std::ffi::OsString;

    #[test]
    fn global_options_do_not_count_as_a_top_level_command() {
        assert!(!has_top_level_command(&[
            OsString::from("--state-root"),
            OsString::from("/tmp/hq"),
        ]));
        assert!(!has_top_level_command(&[
            OsString::from("--output"),
            OsString::from("human"),
        ]));
        assert!(has_top_level_command(&[OsString::from("tui")]));
        assert!(has_top_level_command(&[
            OsString::from("--state-root"),
            OsString::from("/tmp/hq"),
            OsString::from("list"),
        ]));
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
