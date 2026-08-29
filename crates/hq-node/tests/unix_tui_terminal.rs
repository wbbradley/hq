//! Installed terminal routing and pseudoterminal restoration contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    os::fd::OwnedFd,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    pty::{Winsize, openpty},
    sys::termios::{Termios, tcgetattr},
};

use support::TestDirectory;

const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h";
const LEAVE_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049l";

#[test]
fn explicit_and_bare_tui_render_and_restore_the_pseudoterminal() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);

    for explicit in [true, false] {
        let run = run_in_pty(&state_root, explicit);
        assert!(run.status.success(), "TUI process failed: {:?}", run.bytes);
        assert!(
            run.bytes
                .windows(ENTER_ALTERNATE_SCREEN.len())
                .any(|window| window == ENTER_ALTERNATE_SCREEN),
            "TUI did not enter the alternate screen"
        );
        assert!(
            run.bytes.windows(2).any(|window| window == b"HQ"),
            "TUI did not render its title"
        );
        assert!(
            run.bytes
                .windows(LEAVE_ALTERNATE_SCREEN.len())
                .any(|window| window == LEAVE_ALTERNATE_SCREEN),
            "TUI did not leave the alternate screen"
        );
        assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
    }
}

#[test]
fn explicit_tui_without_terminal_fails_without_escape_sequences() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--state-root")
        .arg(state_root)
        .arg("tui")
        .stdin(Stdio::null())
        .output()
        .expect("nonterminal TUI invocation runs");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tui.terminal_required"));
    assert!(!output.stderr.contains(&0x1b));
}

struct PtyRun {
    status: ExitStatus,
    bytes: Vec<u8>,
    before: Termios,
    after: Termios,
}

fn run_in_pty(state_root: &Path, explicit: bool) -> PtyRun {
    let dimensions = Winsize {
        ws_row: 30,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pair = openpty(Some(&dimensions), None).expect("pseudoterminal opens");
    let before = tcgetattr(&pair.slave).expect("initial terminal modes");
    let stdin = stdio_clone(&pair.slave);
    let stdout = stdio_clone(&pair.slave);
    let stderr = stdio_clone(&pair.slave);
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command
        .arg("--state-root")
        .arg(state_root)
        .env("TERM", "xterm-256color")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    if explicit {
        command.arg("tui");
    }
    let mut child = command.spawn().expect("TUI process starts");
    let mut master = File::from(pair.master);
    fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).expect("master is nonblocking");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut bytes = Vec::new();
    let mut quit_sent = false;
    let status = loop {
        let mut buffer = [0_u8; 8192];
        match master.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) if error.raw_os_error() == Some(5) => {}
            Err(error) => panic!("pseudoterminal read failed: {error}"),
        }
        if !quit_sent
            && bytes
                .windows(ENTER_ALTERNATE_SCREEN.len())
                .any(|window| window == ENTER_ALTERNATE_SCREEN)
        {
            master.write_all(b"q").expect("quit key writes");
            master.flush().expect("quit key flushes");
            quit_sent = true;
        }
        if let Some(status) = child.try_wait().expect("TUI process status") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("TUI process timed out; bytes: {bytes:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    loop {
        let mut buffer = [0_u8; 8192];
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(5) =>
            {
                break;
            }
            Err(error) => panic!("final pseudoterminal read failed: {error}"),
        }
    }
    let after = tcgetattr(&pair.slave).expect("restored terminal modes");
    PtyRun {
        status,
        bytes,
        before,
        after,
    }
}

fn stdio_clone(descriptor: &OwnedFd) -> Stdio {
    Stdio::from(File::from(
        descriptor.try_clone().expect("terminal descriptor clones"),
    ))
}

fn initialize_identity(state_root: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(state_root)
        .arg("identity")
        .arg("init")
        .stdin(Stdio::null())
        .output()
        .expect("identity initialization runs");
    assert!(
        output.status.success(),
        "identity initialization failed: {:?}",
        output.stderr
    );
}

struct DaemonStopGuard<'state>(&'state Path);

impl Drop for DaemonStopGuard<'_> {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_hq"))
            .arg("--state-root")
            .arg(self.0)
            .arg("daemon")
            .arg("stop")
            .stdin(Stdio::null())
            .output();
    }
}
