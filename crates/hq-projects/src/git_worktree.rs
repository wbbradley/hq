//! Bounded Git worktree mutation adapter with exact reconciliation.

use std::{
    collections::BTreeMap,
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use hq_application::{ApplicationError, ApplicationErrorCode, EffectOutcome, EffectRequest};
use hq_domain::{
    BoundedText, DomainError, ErrorCategory, ErrorCode, ResourceLocator, ResourceScheme,
};
use hq_resources::{GitCommandOutput, GitRunner};

use crate::{GitWorktreePort, GitWorktreeRequest, GitWorktreeState};

/// Bounds retained per-repository serialization state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitWorktreeAdapterConfig {
    /// Maximum repositories concurrently participating in worktree mutation.
    pub max_repository_locks: NonZeroUsize,
}

impl Default for GitWorktreeAdapterConfig {
    fn default() -> Self {
        Self {
            max_repository_locks: NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// Exact bounded Git adapter; read-only path identity remains owned by hq-resources.
pub struct GitWorktreeAdapter<G> {
    config: GitWorktreeAdapterConfig,
    git: G,
    repository_locks: Mutex<BTreeMap<ResourceLocator, Arc<Mutex<()>>>>,
}

impl<G> GitWorktreeAdapter<G> {
    /// Owns one injected bounded Git runner and repository-lock bound.
    pub const fn new(config: GitWorktreeAdapterConfig, git: G) -> Self {
        Self {
            config,
            git,
            repository_locks: Mutex::new(BTreeMap::new()),
        }
    }
}

impl<G: GitRunner> GitWorktreePort for GitWorktreeAdapter<G> {
    fn lookup(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<GitWorktreeState>, ApplicationError> {
        self.inspect(&request.body)
            .map(EffectOutcome::Accepted)
            .or_else(|error| Ok(EffectOutcome::Rejected(error)))
    }

    fn create(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        let common = self.source_common_directory(&request.body)?;
        let repository_lock = self.repository_lock(&common)?;
        let result = {
            let _guard = repository_lock
                .lock()
                .map_err(|_| unavailable_application())?;
            match self.inspect(&request.body) {
                Ok(GitWorktreeState::Created) => Ok(EffectOutcome::Accepted(())),
                Ok(GitWorktreeState::ReadyToCreate) => {
                    let output = self.run_create(&request.body);
                    match (output, self.inspect(&request.body)) {
                        (_, Ok(GitWorktreeState::Created)) => Ok(EffectOutcome::Accepted(())),
                        (Ok(output), Ok(GitWorktreeState::ReadyToCreate)) if !output.success => {
                            Ok(EffectOutcome::Rejected(domain_error(
                                ErrorCategory::Conflict,
                                "project_git_create_rejected",
                            )))
                        }
                        _ => Ok(EffectOutcome::Uncertain(request.operation_id)),
                    }
                }
                Err(_) => Ok(EffectOutcome::Rejected(domain_error(
                    ErrorCategory::Conflict,
                    "project_git_state_conflict",
                ))),
            }
        };
        self.release_repository_lock(&common, &repository_lock)?;
        result
    }
}

impl<G: GitRunner> GitWorktreeAdapter<G> {
    fn inspect(&self, request: &GitWorktreeRequest) -> Result<GitWorktreeState, DomainError> {
        let source = locator_path(&request.source, true)?;
        let destination = locator_path(&request.destination, false)?;
        if !self.valid_branch(&source, request.branch.as_str()) {
            return Err(domain_error(
                ErrorCategory::InvalidInput,
                "project_git_branch_invalid",
            ));
        }
        let source_common = self
            .source_common_directory_from(&source)
            .map_err(|_| domain_error(ErrorCategory::InvalidInput, "project_git_source_invalid"))?;
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(
                domain_error(ErrorCategory::Conflict, "project_git_destination_conflict"),
            ),
            Ok(_) => {
                let canonical_destination = fs::canonicalize(&destination).map_err(|_| {
                    domain_error(
                        ErrorCategory::Unresolved,
                        "project_git_destination_unavailable",
                    )
                })?;
                let worktree = self
                    .git_path(&destination, &["rev-parse", "--show-toplevel"])
                    .ok_or_else(|| {
                        domain_error(
                            ErrorCategory::Conflict,
                            "project_git_destination_not_worktree",
                        )
                    })?;
                let worktree = fs::canonicalize(worktree).map_err(|_| {
                    domain_error(
                        ErrorCategory::Unresolved,
                        "project_git_destination_unavailable",
                    )
                })?;
                let common = self.source_common_directory_from(&destination)?;
                let branch = self
                    .git_text(
                        &destination,
                        &["symbolic-ref", "--quiet", "--short", "HEAD"],
                    )
                    .ok_or_else(|| {
                        domain_error(ErrorCategory::Conflict, "project_git_detached_worktree")
                    })?;
                if worktree == canonical_destination
                    && common == source_common
                    && branch == request.branch.as_str()
                {
                    Ok(GitWorktreeState::Created)
                } else {
                    Err(domain_error(
                        ErrorCategory::Conflict,
                        "project_git_worktree_mismatch",
                    ))
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.ready_without_destination(request, &source, &destination)
            }
            Err(_) => Err(domain_error(
                ErrorCategory::Unresolved,
                "project_git_destination_unavailable",
            )),
        }
    }

    fn ready_without_destination(
        &self,
        request: &GitWorktreeRequest,
        source: &Path,
        destination: &Path,
    ) -> Result<GitWorktreeState, DomainError> {
        let registrations = self.worktree_registrations(source)?;
        if registrations
            .iter()
            .any(|registration| registration.path == destination)
        {
            return Err(domain_error(
                ErrorCategory::Conflict,
                "project_git_registration_conflict",
            ));
        }
        let branch_registered = registrations
            .iter()
            .any(|registration| registration.branch.as_deref() == Some(request.branch.as_str()));
        let reference = format!("refs/heads/{}", request.branch.as_str());
        let branch_exists = self
            .git
            .run(source, &["show-ref", "--verify", "--quiet", &reference])
            .map(|output| output.success)
            .map_err(|_| {
                domain_error(
                    ErrorCategory::Unresolved,
                    "project_git_branch_lookup_unknown",
                )
            })?;
        let safe = if request.create_branch {
            !branch_exists && !branch_registered
        } else {
            branch_exists && !branch_registered
        };
        if safe {
            Ok(GitWorktreeState::ReadyToCreate)
        } else {
            Err(domain_error(
                ErrorCategory::Conflict,
                "project_git_branch_conflict",
            ))
        }
    }

    fn source_common_directory(
        &self,
        request: &GitWorktreeRequest,
    ) -> Result<ResourceLocator, ApplicationError> {
        let source = locator_path(&request.source, true).map_err(|_| invalid_application())?;
        self.source_common_directory_from(&source)
            .map_err(|_| invalid_application())
    }

    fn source_common_directory_from(&self, source: &Path) -> Result<ResourceLocator, DomainError> {
        let path = self
            .git_path(
                source,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .ok_or_else(|| {
                domain_error(ErrorCategory::InvalidInput, "project_git_source_invalid")
            })?;
        let canonical = fs::canonicalize(path).map_err(|_| {
            domain_error(ErrorCategory::Unresolved, "project_git_source_unavailable")
        })?;
        let text = canonical.to_str().ok_or_else(|| {
            domain_error(ErrorCategory::InvalidInput, "project_git_path_encoding")
        })?;
        let value = BoundedText::new(text.to_owned())
            .map_err(|_| domain_error(ErrorCategory::InvalidInput, "project_git_path_too_long"))?;
        Ok(ResourceLocator::new(ResourceScheme::GitRepository, value))
    }

    fn valid_branch(&self, source: &Path, branch: &str) -> bool {
        self.git
            .run(source, &["check-ref-format", "--branch", branch])
            .is_ok_and(|output| output.success)
    }

    fn run_create(
        &self,
        request: &GitWorktreeRequest,
    ) -> Result<GitCommandOutput, hq_resources::GitCommandFailure> {
        let source = Path::new(request.source.value());
        if request.create_branch {
            self.git.run(
                source,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    request.branch.as_str(),
                    request.destination.value(),
                ],
            )
        } else {
            self.git.run(
                source,
                &[
                    "worktree",
                    "add",
                    "-q",
                    request.destination.value(),
                    request.branch.as_str(),
                ],
            )
        }
    }

    fn worktree_registrations(&self, source: &Path) -> Result<Vec<Registration>, DomainError> {
        let output = self
            .git
            .run(source, &["worktree", "list", "--porcelain", "-z"])
            .map_err(|_| {
                domain_error(
                    ErrorCategory::Unresolved,
                    "project_git_registration_lookup_unknown",
                )
            })?;
        if !output.success {
            return Err(domain_error(
                ErrorCategory::Unresolved,
                "project_git_registration_lookup_unknown",
            ));
        }
        parse_registrations(&output.stdout)
    }

    fn git_path(&self, directory: &Path, arguments: &[&str]) -> Option<PathBuf> {
        self.git_text(directory, arguments).map(PathBuf::from)
    }

    fn git_text(&self, directory: &Path, arguments: &[&str]) -> Option<String> {
        let output = self.git.run(directory, arguments).ok()?;
        if !output.success {
            return None;
        }
        let value = std::str::from_utf8(&output.stdout).ok()?;
        let value = value.strip_suffix('\n').unwrap_or(value);
        let value = value.strip_suffix('\r').unwrap_or(value);
        (!value.is_empty() && !value.contains(['\n', '\r', '\0'])).then(|| value.to_owned())
    }

    fn repository_lock(
        &self,
        repository: &ResourceLocator,
    ) -> Result<Arc<Mutex<()>>, ApplicationError> {
        let mut locks = self
            .repository_locks
            .lock()
            .map_err(|_| unavailable_application())?;
        if let Some(lock) = locks.get(repository) {
            return Ok(Arc::clone(lock));
        }
        if locks.len() >= self.config.max_repository_locks.get() {
            return Err(ApplicationError::new(ApplicationErrorCode::IntakeFull));
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(repository.clone(), Arc::clone(&lock));
        Ok(lock)
    }

    fn release_repository_lock(
        &self,
        repository: &ResourceLocator,
        lock: &Arc<Mutex<()>>,
    ) -> Result<(), ApplicationError> {
        let mut locks = self
            .repository_locks
            .lock()
            .map_err(|_| unavailable_application())?;
        if Arc::strong_count(lock) == 2
            && locks
                .get(repository)
                .is_some_and(|current| Arc::ptr_eq(current, lock))
        {
            locks.remove(repository);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Registration {
    path: PathBuf,
    branch: Option<String>,
}

fn parse_registrations(bytes: &[u8]) -> Result<Vec<Registration>, DomainError> {
    let mut registrations = Vec::new();
    let mut current: Option<Registration> = None;
    for field in bytes.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let Some(registration) = current.take() {
                registrations.push(registration);
            }
            continue;
        }
        let text = std::str::from_utf8(field).map_err(|_| {
            domain_error(
                ErrorCategory::Unresolved,
                "project_git_registration_malformed",
            )
        })?;
        if let Some(path) = text.strip_prefix("worktree ") {
            if let Some(registration) = current.take() {
                registrations.push(registration);
            }
            current = Some(Registration {
                path: PathBuf::from(path),
                branch: None,
            });
        } else if let Some(branch) = text.strip_prefix("branch refs/heads/") {
            let Some(registration) = current.as_mut() else {
                return Err(domain_error(
                    ErrorCategory::Unresolved,
                    "project_git_registration_malformed",
                ));
            };
            registration.branch = Some(branch.to_owned());
        }
    }
    if let Some(registration) = current {
        registrations.push(registration);
    }
    if registrations.iter().any(|registration| {
        !registration.path.is_absolute()
            || registration.branch.as_ref().is_some_and(String::is_empty)
    }) {
        return Err(domain_error(
            ErrorCategory::Unresolved,
            "project_git_registration_malformed",
        ));
    }
    Ok(registrations)
}

fn locator_path(locator: &ResourceLocator, source: bool) -> Result<PathBuf, DomainError> {
    let valid_scheme = if source {
        matches!(
            locator.scheme(),
            ResourceScheme::WorkingTree | ResourceScheme::GitRepository
        )
    } else {
        locator.scheme() == ResourceScheme::WorkingTree
    };
    let path = PathBuf::from(locator.value());
    if valid_scheme && path.is_absolute() {
        Ok(path)
    } else {
        Err(domain_error(
            ErrorCategory::InvalidInput,
            "project_git_locator_invalid",
        ))
    }
}

fn invalid_application() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}

fn unavailable_application() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::AdapterUnavailable)
}

#[allow(clippy::expect_used, reason = "all callers pass reviewed static codes")]
fn domain_error(category: ErrorCategory, code: &'static str) -> DomainError {
    DomainError::new(
        category,
        ErrorCode::new(code).expect("static Git worktree error code"),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::parse_registrations;

    #[test]
    fn porcelain_registration_parser_is_closed_and_exact() {
        let parsed = parse_registrations(
            b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0worktree /repo/wt\0HEAD def\0detached\0\0",
        )
        .expect("valid porcelain");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].branch.as_deref(), Some("main"));
        assert_eq!(parsed[1].branch, None);
        assert!(parse_registrations(b"branch refs/heads/main\0").is_err());
        assert!(parse_registrations(b"worktree relative\0\0").is_err());
    }
}
