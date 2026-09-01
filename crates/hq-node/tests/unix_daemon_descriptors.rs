//! Daemon process descriptor-isolation contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    io::Read as _,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use hq_node::{RuntimePaths, StateDirectoryOwner, StatePaths};
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    unistd::{pipe, read},
};

use support::TestDirectory;

#[test]
fn foreground_daemon_closes_an_inherited_nonstandard_descriptor() {
    let directory = TestDirectory::new();
    let state = initialize(directory.path());
    let runtime = RuntimePaths::new(state.root().join("runtime")).expect("runtime paths");
    let (read_end, write_end) = pipe().expect("inheritable pipe opens");
    fcntl(&read_end, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).expect("read end is nonblocking");

    let mut child = ChildGuard(Some(
        Command::new(env!("CARGO_BIN_EXE_hq"))
            .arg("--state-root")
            .arg(state.root())
            .arg("daemon")
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("foreground daemon starts"),
    ));
    drop(write_end);
    child.wait_for_path(runtime.readiness_file(), Duration::from_secs(10));

    let mut byte = [0_u8; 1];
    assert_eq!(
        read(&read_end, &mut byte),
        Ok(0),
        "a live daemon must not retain the caller's pipe"
    );

    let stopped = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--state-root")
        .arg(state.root())
        .arg("daemon")
        .arg("stop")
        .stdin(Stdio::null())
        .output()
        .expect("daemon stop runs");
    assert!(
        stopped.status.success(),
        "stop failed: {:?}",
        stopped.stderr
    );
    child.wait(Duration::from_secs(10));
}

fn initialize(root: &Path) -> StatePaths {
    let state = StatePaths::new(root.join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity initializes");
    drop(owner);
    state
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn wait_for_path(&mut self, path: &Path, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if path.exists() {
                return;
            }
            let child = self.0.as_mut().expect("guarded child");
            if let Some(status) = child.try_wait().expect("child status") {
                let mut stderr = String::new();
                if let Some(mut stream) = child.stderr.take() {
                    stream
                        .read_to_string(&mut stderr)
                        .expect("daemon stderr reads");
                }
                panic!("daemon exited before readiness with {status}: {stderr}");
            }
            assert!(Instant::now() < deadline, "daemon readiness timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .0
                .as_mut()
                .expect("guarded child")
                .try_wait()
                .expect("child status")
                .is_some()
            {
                let _ = self.0.take();
                return;
            }
            assert!(Instant::now() < deadline, "daemon did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
