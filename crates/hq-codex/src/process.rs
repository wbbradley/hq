//! Child-process and provider-private launch policy ownership.

use std::{
    ffi::OsString,
    fmt,
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::{ffi::OsStringExt, process::CommandExt as _};

use hq_harness::{HarnessEnvironment, HarnessError, HarnessErrorClass, HarnessInstanceRequest};

/// Provider-private local launch values selected from neutral agent/project identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexLaunch {
    /// Absolute or process-relative executable spelling.
    pub executable: PathBuf,
    /// Working directory supplied to app-server and thread operations.
    pub working_directory: PathBuf,
    /// Instructions attached only when starting a fresh thread.
    pub developer_instructions: String,
    /// Optional model override; absence lets Codex configuration select it.
    pub model: Option<String>,
    /// Whether to request explicit local permissive execution settings.
    pub permissive: bool,
}

/// Resolves provider-private launch values without extending the neutral request vocabulary.
pub trait CodexLaunchResolver: Send + Sync {
    /// Resolves one logical instance's local launch policy.
    fn resolve(&self, request: &HarnessInstanceRequest) -> Result<CodexLaunch, HarnessError>;
}

/// Fixed launch policy useful for direct named-agent workers and tests.
#[derive(Clone, Debug)]
pub struct FixedCodexLaunchResolver {
    /// Launch values returned for every neutral instance request.
    pub launch: CodexLaunch,
}

impl CodexLaunchResolver for FixedCodexLaunchResolver {
    fn resolve(&self, _request: &HarnessInstanceRequest) -> Result<CodexLaunch, HarnessError> {
        Ok(self.launch.clone())
    }
}

/// Bounded process wait observation without child diagnostic text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexWaitOutcome {
    /// The child exited successfully.
    ExitedSuccessfully,
    /// The child exited unsuccessfully.
    ExitedUnsuccessfully,
    /// The supplied bounded wait expired while it was still running.
    Running,
}

/// Opaque child lifetime capability shared with adapter shutdown.
pub trait CodexProcessControl: Send + Sync {
    /// Observes exit for at most `wait`.
    fn wait(&self, wait: Duration) -> Result<CodexWaitOutcome, HarnessError>;

    /// Idempotently requests immediate termination of the owned runtime process group.
    fn kill(&self) -> Result<(), HarnessError>;
}

/// Complete owned stdio and lifetime capabilities returned by one child start.
pub struct CodexProcessPipes {
    /// Child stdin, owned by the JSONL writer.
    pub input: Box<dyn Write + Send>,
    /// Child stdout, owned by the bounded JSONL reader.
    pub output: Box<dyn Read + Send>,
    /// Child stderr, owned only by the private diagnostic drain.
    pub errors: Box<dyn Read + Send>,
    /// Checked wait/kill capability.
    pub control: Arc<dyn CodexProcessControl>,
}

impl fmt::Debug for CodexProcessPipes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexProcessPipes")
            .finish_non_exhaustive()
    }
}

/// Injectable process creation capability.
pub trait CodexProcessStarter: Send + Sync {
    /// Starts exactly one app-server child with a copied memory-only environment.
    fn start(
        &self,
        launch: &CodexLaunch,
        environment: &HarnessEnvironment,
    ) -> Result<CodexProcessPipes, HarnessError>;
}

/// Private sink for untrusted provider stderr lines.
pub trait CodexDiagnosticSink: Send + Sync {
    /// Observes one bounded line without feeding it into neutral control flow.
    fn line(&self, line: &str);
}

/// Body-free operational diagnostics for the bounded app-server transport.
pub trait CodexOperationalDiagnosticSink: Send + Sync {
    /// Records adjacent replaceable notifications discarded before queue admission.
    fn transport_coalesced(&self, count: usize);

    /// Records one validated provider interaction entering the adapter.
    fn interaction_received(&self, operation_id: [u8; 32], request_id: [u8; 32]);
}

/// Default diagnostic sink that discards all provider stderr.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiscardCodexDiagnostics;

impl CodexDiagnosticSink for DiscardCodexDiagnostics {
    fn line(&self, _line: &str) {}
}

impl CodexOperationalDiagnosticSink for DiscardCodexDiagnostics {
    fn transport_coalesced(&self, _count: usize) {}

    fn interaction_received(&self, _operation_id: [u8; 32], _request_id: [u8; 32]) {}
}

/// Standard-library app-server process starter.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExecCodexProcessStarter;

impl CodexProcessStarter for ExecCodexProcessStarter {
    fn start(
        &self,
        launch: &CodexLaunch,
        environment: &HarnessEnvironment,
    ) -> Result<CodexProcessPipes, HarnessError> {
        let mut command = Command::new(&launch.executable);
        command
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .current_dir(&launch.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(not(unix))]
        let mut invalid_environment = false;
        environment.visit(|name, value| {
            #[cfg(unix)]
            command.env(name, OsString::from_vec(value.to_vec()));
            #[cfg(not(unix))]
            match std::str::from_utf8(value) {
                Ok(value) => {
                    command.env(name, value);
                }
                Err(_) => invalid_environment = true,
            }
        });
        #[cfg(not(unix))]
        if invalid_environment {
            return Err(HarnessError::new(HarnessErrorClass::InvalidInput));
        }
        let mut child = command
            .spawn()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        let errors = child
            .stderr
            .take()
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::Unavailable))?;
        Ok(CodexProcessPipes {
            input: Box::new(input),
            output: Box::new(output),
            errors: Box::new(errors),
            control: Arc::new(ChildControl {
                state: Mutex::new(ChildState {
                    child,
                    outcome: None,
                    group_terminated: false,
                }),
            }),
        })
    }
}

struct ChildControl {
    state: Mutex<ChildState>,
}

struct ChildState {
    child: Child,
    outcome: Option<CodexWaitOutcome>,
    group_terminated: bool,
}

impl ChildState {
    fn observe_exit(&mut self) -> Result<Option<CodexWaitOutcome>, HarnessError> {
        if self.outcome.is_some() {
            return Ok(self.outcome);
        }
        #[cfg(unix)]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            // Keep the group leader unreaped until cleanup: its PID pins the group
            // identity so retries cannot signal a later, unrelated process group.
            let observed = loop {
                match waitid(
                    WaitId::Pid(Pid::from_child(&self.child)),
                    WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
                ) {
                    Err(rustix::io::Errno::INTR) => {}
                    result => {
                        break result
                            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
                    }
                }
            };
            if observed.is_none() {
                return Ok(None);
            }
            self.terminate_group()?;
        }
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?
        {
            self.outcome = Some(if status.success() {
                CodexWaitOutcome::ExitedSuccessfully
            } else {
                CodexWaitOutcome::ExitedUnsuccessfully
            });
        }
        Ok(self.outcome)
    }

    fn terminate_group(&mut self) -> Result<(), HarnessError> {
        if self.group_terminated || self.outcome.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            use rustix::process::{Pid, Signal, kill_process_group};
            match kill_process_group(Pid::from_child(&self.child), Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                Err(_) => return Err(HarnessError::new(HarnessErrorClass::CleanupFailed)),
            }
        }
        #[cfg(not(unix))]
        if self
            .child
            .try_wait()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?
            .is_none()
        {
            self.child
                .kill()
                .map_err(|_| HarnessError::new(HarnessErrorClass::CleanupFailed))?;
        }
        self.group_terminated = true;
        Ok(())
    }
}

impl CodexProcessControl for ChildControl {
    fn wait(&self, wait: Duration) -> Result<CodexWaitOutcome, HarnessError> {
        let deadline = Instant::now()
            .checked_add(wait)
            .ok_or_else(|| HarnessError::new(HarnessErrorClass::InvalidInput))?;
        loop {
            let outcome = self
                .state
                .lock()
                .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?
                .observe_exit()?;
            if let Some(outcome) = outcome {
                return Ok(outcome);
            }
            if Instant::now() >= deadline {
                return Ok(CodexWaitOutcome::Running);
            }
            thread::sleep(Duration::from_millis(5).min(wait));
        }
    }

    fn kill(&self) -> Result<(), HarnessError> {
        self.state
            .lock()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?
            .terminate_group()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        io::Read as _,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use hq_harness::HarnessEnvironment;

    use super::{CodexLaunch, CodexProcessStarter, CodexWaitOutcome, ExecCodexProcessStarter};

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn executable_receives_only_the_copied_environment_and_exact_arguments()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = temporary_directory()?;
        let script = directory.join("app-server");
        fs::write(
            &script,
            "printf '%s\\n' \"$HQ_TEST_VALUE\"\nprintf '%s\\n' \"${AMBIENT_SHOULD_NOT_LEAK-unset}\"\nprintf '%s|%s\\n' \"$1\" \"$2\"\n",
        )?;
        let launch = CodexLaunch {
            executable: PathBuf::from("/bin/sh"),
            working_directory: directory.clone(),
            developer_instructions: "unused by process start".to_owned(),
            model: None,
            permissive: false,
        };
        let environment = HarnessEnvironment::copy_from([("HQ_TEST_VALUE", b"copied".as_slice())])?;
        let mut pipes = ExecCodexProcessStarter.start(&launch, &environment)?;
        drop(pipes.input);
        let mut output = String::new();
        pipes.output.read_to_string(&mut output)?;
        let outcome = pipes.control.wait(Duration::from_secs(1))?;
        assert_eq!(outcome, CodexWaitOutcome::ExitedSuccessfully);
        pipes.control.kill()?;
        pipes.control.kill()?;
        assert_eq!(output, "copied\nunset\n--listen|stdio://\n");
        fs::remove_file(script)?;
        fs::remove_dir(directory)?;
        Ok(())
    }

    #[test]
    fn forced_cleanup_stops_descendants_without_touching_an_unrelated_process()
    -> Result<(), Box<dyn std::error::Error>> {
        exercise_process_tree_cleanup(false)
    }

    #[test]
    fn observing_leader_exit_cleans_descendants_before_caching_terminal_status()
    -> Result<(), Box<dyn std::error::Error>> {
        exercise_process_tree_cleanup(true)
    }

    fn exercise_process_tree_cleanup(leader_exits: bool) -> Result<(), Box<dyn std::error::Error>> {
        let directory = temporary_directory()?;
        fs::write(
            directory.join("app-server"),
            r"
/bin/sh -c '/bin/sleep 60 & echo $! > grandchild.pid; echo $$ > child.pid; wait' &
echo $$ > leader.pid
while ! test -s grandchild.pid || ! test -s child.pid; do /bin/sleep 0.01; done
printf ready > ready
if test -f exit-leader; then exit 0; fi
wait
",
        )?;
        if leader_exits {
            fs::write(directory.join("exit-leader"), b"exit")?;
        }
        let launch = CodexLaunch {
            executable: PathBuf::from("/bin/sh"),
            working_directory: directory.clone(),
            developer_instructions: String::new(),
            model: None,
            permissive: false,
        };
        let pipes = ExecCodexProcessStarter.start(&launch, &HarnessEnvironment::default())?;
        let mut fixture = ProcessTreeFixture {
            directory,
            control: std::sync::Arc::clone(&pipes.control),
            unrelated: std::process::Command::new("/bin/sleep").arg("60").spawn()?,
        };
        assert!(
            wait_until(|| fixture.directory.join("ready").exists()),
            "descendants become ready"
        );
        let child = fs::read_to_string(fixture.directory.join("child.pid"))?;
        let grandchild = fs::read_to_string(fixture.directory.join("grandchild.pid"))?;
        assert!(process_is_running(child.trim()));
        assert!(process_is_running(grandchild.trim()));
        if !leader_exits {
            assert_eq!(
                pipes.control.wait(Duration::from_millis(10))?,
                CodexWaitOutcome::Running
            );
            pipes.control.kill()?;
        }
        let outcome = pipes.control.wait(Duration::from_secs(2))?;
        assert_eq!(
            outcome,
            if leader_exits {
                CodexWaitOutcome::ExitedSuccessfully
            } else {
                CodexWaitOutcome::ExitedUnsuccessfully
            }
        );
        assert!(
            wait_until(
                || !process_is_running(child.trim()) && !process_is_running(grandchild.trim())
            ),
            "cleanup stops both generations of descendants"
        );
        pipes.control.kill()?;
        pipes.control.kill()?;
        assert_eq!(pipes.control.wait(Duration::ZERO)?, outcome);
        assert!(
            fixture.unrelated.try_wait()?.is_none(),
            "unrelated process survives cleanup and retries"
        );
        Ok(())
    }

    struct ProcessTreeFixture {
        directory: PathBuf,
        control: std::sync::Arc<dyn super::CodexProcessControl>,
        unrelated: std::process::Child,
    }

    impl Drop for ProcessTreeFixture {
        fn drop(&mut self) {
            // Also clean descendants when a pre-fix assertion fails.
            for name in ["grandchild.pid", "child.pid"] {
                if let Ok(pid) = fs::read_to_string(self.directory.join(name)) {
                    let _ = std::process::Command::new("/bin/kill")
                        .arg("-KILL")
                        .arg(pid.trim())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
            let _ = self.control.kill();
            let _ = self.control.wait(Duration::from_secs(1));
            let _ = self.unrelated.kill();
            let _ = self.unrelated.wait();
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn process_is_running(pid: &str) -> bool {
        let Ok(output) = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
        else {
            return true;
        };
        let status = String::from_utf8_lossy(&output.stdout);
        // Orphaned zombies have terminated; only their OS-owned reaping remains.
        output.status.success()
            && !status.trim().is_empty()
            && !status.trim_start().starts_with('Z')
    }

    fn wait_until(mut observed: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if observed() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn temporary_directory() -> Result<PathBuf, std::io::Error> {
        let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::SeqCst);
        let directory =
            std::env::temp_dir().join(format!("hq-codex-process-{}-{suffix}", std::process::id()));
        fs::create_dir(&directory)?;
        Ok(directory)
    }
}
