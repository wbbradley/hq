//! Bounded Git subprocess transport and release assessment values.

use std::{
    collections::BTreeSet,
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use hq_domain::{InstallationId, ResourceId, ResourceLocator};

/// Passive bounded Git command configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommandConfig {
    /// Executable spelling resolved by the process environment.
    pub executable: PathBuf,
    /// Maximum wall time for one read-only command.
    pub timeout: Duration,
    /// Inclusive stdout byte bound.
    pub max_output_bytes: usize,
}

/// Closed Git subprocess failure without command output or environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitCommandFailure {
    /// Process creation failed.
    Unavailable,
    /// The configured wall deadline expired.
    TimedOut,
    /// Captured stdout exceeded its inclusive bound.
    OutputTooLarge,
    /// Captured stdout could not be read completely.
    ReadFailed,
    /// Process status could not be observed or cleaned up.
    WaitFailed,
}

/// Bounded command completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommandOutput {
    /// Whether Git returned a successful status.
    pub success: bool,
    /// Bounded stdout; stderr is never retained.
    pub stdout: Vec<u8>,
}

/// Injectable read-only Git command capability.
pub trait GitRunner: Clone + Send + Sync + 'static {
    /// Runs `git -C directory` with exact read-only arguments.
    fn run(
        &self,
        directory: &std::path::Path,
        arguments: &[&str],
    ) -> Result<GitCommandOutput, GitCommandFailure>;
}

/// Standard bounded Git subprocess runner.
#[derive(Clone, Debug)]
pub struct ExecGit {
    config: GitCommandConfig,
}

impl ExecGit {
    /// Validates and owns exact process bounds.
    pub fn new(config: GitCommandConfig) -> Result<Self, GitCommandFailure> {
        if config.executable.as_os_str().is_empty()
            || config.timeout.is_zero()
            || config.max_output_bytes == 0
        {
            return Err(GitCommandFailure::Unavailable);
        }
        Ok(Self { config })
    }

    pub(crate) fn system() -> Self {
        Self {
            config: GitCommandConfig {
                executable: PathBuf::from("git"),
                timeout: Duration::from_secs(5),
                max_output_bytes: 1024 * 1024,
            },
        }
    }
}

impl GitRunner for ExecGit {
    fn run(
        &self,
        directory: &std::path::Path,
        arguments: &[&str],
    ) -> Result<GitCommandOutput, GitCommandFailure> {
        let deadline = Instant::now()
            .checked_add(self.config.timeout)
            .ok_or(GitCommandFailure::TimedOut)?;
        let mut command = Command::new(&self.config.executable);
        command
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_NAMESPACE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_process_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| GitCommandFailure::Unavailable)?;
        let process_group = child.id();
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitCommandFailure::ReadFailed);
        };
        let maximum = self.config.max_output_bytes;
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    terminate_process_tree(&mut child, process_group);
                    let _ = reader.join();
                    return Err(GitCommandFailure::TimedOut);
                }
                Err(_) => {
                    terminate_process_tree(&mut child, process_group);
                    let _ = reader.join();
                    return Err(GitCommandFailure::WaitFailed);
                }
            }
        };
        while !reader.is_finished() {
            if Instant::now() >= deadline {
                terminate_process_group(process_group);
                let _ = reader.join();
                return Err(GitCommandFailure::TimedOut);
            }
            thread::sleep(Duration::from_millis(5));
        }
        let stdout = reader
            .join()
            .map_err(|_| GitCommandFailure::ReadFailed)?
            .map_err(|_| GitCommandFailure::ReadFailed)?;
        if stdout.len() > maximum {
            return Err(GitCommandFailure::OutputTooLarge);
        }
        Ok(GitCommandOutput {
            success: status.success(),
            stdout,
        })
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

fn terminate_process_tree(child: &mut Child, process_group: u32) {
    terminate_process_group(process_group);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_process_group(process_group: u32) {
    let group = format!("-{process_group}");
    let _ = Command::new("/bin/kill")
        .arg("-KILL")
        .arg(group)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
const fn terminate_process_group(_process_group: u32) {}

/// Stable Git worktree change classes used by release policy.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GitChangeKind {
    /// Index differs from `HEAD`.
    Staged,
    /// Worktree differs from the index.
    Unstaged,
    /// A tracked path is deleted in either layer.
    Deleted,
    /// A tracked path is renamed or copied.
    Renamed,
    /// A path is not tracked.
    Untracked,
    /// Git reports an unresolved merge state.
    Unmerged,
}

/// Generic path release state consumed by lifecycle force policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathReleaseState {
    /// Git worktree exists and has no reported changes.
    Clean,
    /// Git worktree contains at least one change.
    Dirty,
    /// Assessment could not establish a safe release result.
    Unknown,
    /// The path is not inside a Git worktree.
    NotApplicable,
}

impl PathReleaseState {
    pub(crate) const fn requires_force(self) -> bool {
        matches!(self, Self::Dirty | Self::Unknown)
    }
}

/// Passive, bounded path release assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathReleaseAssessment {
    /// Home-local namespace qualifying the resource.
    pub home: InstallationId,
    /// Stable resource identity being assessed.
    pub resource_id: ResourceId,
    /// Closed release classification.
    pub state: PathReleaseState,
    /// Distinct Git worktree top-level identity, when established.
    pub worktree_identity: Option<ResourceLocator>,
    /// Shared repository-maintenance identity, when established.
    pub common_git_directory: Option<ResourceLocator>,
    /// Closed change classes without file contents or paths.
    pub changes: BTreeSet<GitChangeKind>,
    /// Number of bounded porcelain entries observed.
    pub changed_entries: usize,
}

impl PathReleaseAssessment {
    pub(crate) fn state(
        home: InstallationId,
        resource_id: ResourceId,
        state: PathReleaseState,
    ) -> Self {
        Self {
            home,
            resource_id,
            state,
            worktree_identity: None,
            common_git_directory: None,
            changes: BTreeSet::new(),
            changed_entries: 0,
        }
    }
}

pub(crate) fn parse_status(bytes: &[u8]) -> Result<(BTreeSet<GitChangeKind>, usize), ()> {
    let mut changes = BTreeSet::new();
    let mut entries = 0usize;
    let records = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut index_record = 0usize;
    while index_record < records.len() {
        let record = records[index_record];
        index_record = index_record.saturating_add(1);
        if record.is_empty() {
            continue;
        }
        if record.len() < 4 || record[2] != b' ' {
            return Err(());
        }
        let index = record[0];
        let worktree = record[1];
        if !valid_status(index) || !valid_status(worktree) {
            return Err(());
        }
        if (matches!(index, b'?' | b'!') || matches!(worktree, b'?' | b'!')) && index != worktree {
            return Err(());
        }
        if index == b' ' && worktree == b' ' {
            return Err(());
        }
        if index == b'?' && worktree == b'?' {
            changes.insert(GitChangeKind::Untracked);
            entries = entries.saturating_add(1);
            continue;
        }
        if index == b'!' && worktree == b'!' {
            continue;
        }
        if matches!(
            (index, worktree),
            (b'D' | b'U', b'D') | (b'A' | b'D' | b'U', b'U') | (b'A' | b'U', b'A')
        ) {
            changes.insert(GitChangeKind::Unmerged);
        }
        if index != b' ' && index != b'!' {
            changes.insert(GitChangeKind::Staged);
        }
        if worktree != b' ' && worktree != b'!' {
            changes.insert(GitChangeKind::Unstaged);
        }
        if index == b'D' || worktree == b'D' {
            changes.insert(GitChangeKind::Deleted);
        }
        let rename = matches!(index, b'R' | b'C') || matches!(worktree, b'R' | b'C');
        if rename {
            changes.insert(GitChangeKind::Renamed);
            let Some(previous_path) = records.get(index_record) else {
                return Err(());
            };
            if previous_path.is_empty() {
                return Err(());
            }
            index_record = index_record.saturating_add(1);
        }
        entries = entries.saturating_add(1);
    }
    Ok((changes, entries))
}

const fn valid_status(value: u8) -> bool {
    matches!(
        value,
        b' ' | b'M' | b'T' | b'A' | b'D' | b'R' | b'C' | b'U' | b'?' | b'!'
    )
}

#[cfg(test)]
mod tests {
    use super::{GitChangeKind, parse_status};

    #[test]
    fn rename_source_is_not_misread_as_another_status_record() {
        let parsed = parse_status(b"R  destination\0M  source-looking-name\0");
        assert_eq!(parsed.as_ref().map(|value| value.1), Ok(1));
        assert_eq!(
            parsed.map(|value| value.0.contains(&GitChangeKind::Renamed)),
            Ok(true)
        );
    }

    #[test]
    fn malformed_or_truncated_porcelain_is_rejected() {
        assert!(parse_status(b"not porcelain\0").is_err());
        assert!(parse_status(b"R  destination\0").is_err());
        assert!(parse_status(b"   clean-should-not-be-emitted\0").is_err());
        assert!(parse_status(b"?  mismatched-special-status\0").is_err());
    }

    #[test]
    fn unmerged_and_ignored_records_have_closed_meaning() {
        let unmerged = parse_status(b"DD conflict\0");
        assert_eq!(
            unmerged.map(|value| value.0.contains(&GitChangeKind::Unmerged)),
            Ok(true)
        );
        assert_eq!(parse_status(b"!! ignored\0").map(|value| value.1), Ok(0));
    }
}
