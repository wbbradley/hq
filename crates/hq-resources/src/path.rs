//! Absolute path normalization and identity observation.

use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use hq_domain::{
    BoundedText, InstallationId, ProjectResource, RESOURCE_LOCATOR_MAX_BYTES, ResourceHealth,
    ResourceId, ResourceLocator, ResourceScheme,
};

/// Closed path observation failure without operating-system diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathProbeError {
    /// The observed entry does not exist.
    Missing,
    /// Traversal or metadata access was denied.
    Inaccessible,
    /// Observation failed for another reason.
    Unknown,
}

/// Followed or unfollowed entry classification needed by the adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathEntryKind {
    /// A directory suitable for a project resource or launch.
    Directory,
    /// A symbolic link before following its target.
    SymbolicLink,
    /// An existing non-directory entry.
    Other,
}

/// Injectable read-only filesystem observation capability.
pub trait PathSystem: Clone + Send + Sync + 'static {
    /// Observes one entry without following the final symbolic link.
    fn symlink_entry(&self, path: &Path) -> Result<PathEntryKind, PathProbeError>;

    /// Resolves one existing path through symbolic links.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, PathProbeError>;

    /// Observes the final target after following symbolic links.
    fn followed_entry(&self, path: &Path) -> Result<PathEntryKind, PathProbeError>;
}

/// Standard-library read-only filesystem capability.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdPathSystem;

impl PathSystem for StdPathSystem {
    fn symlink_entry(&self, path: &Path) -> Result<PathEntryKind, PathProbeError> {
        fs::symlink_metadata(path)
            .map(|metadata| entry_kind(&metadata))
            .map_err(|error| probe_error(&error))
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, PathProbeError> {
        fs::canonicalize(path).map_err(|error| probe_error(&error))
    }

    fn followed_entry(&self, path: &Path) -> Result<PathEntryKind, PathProbeError> {
        fs::metadata(path)
            .map(|metadata| entry_kind(&metadata))
            .map_err(|error| probe_error(&error))
    }
}

fn entry_kind(metadata: &fs::Metadata) -> PathEntryKind {
    if metadata.file_type().is_symlink() {
        PathEntryKind::SymbolicLink
    } else if metadata.is_dir() {
        PathEntryKind::Directory
    } else {
        PathEntryKind::Other
    }
}

fn probe_error(error: &std::io::Error) -> PathProbeError {
    match error.kind() {
        std::io::ErrorKind::NotFound => PathProbeError::Missing,
        std::io::ErrorKind::PermissionDenied => PathProbeError::Inaccessible,
        _ => PathProbeError::Unknown,
    }
}

/// Exact resource condition retained independently from coarse semantic health.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathCondition {
    /// The recorded path exists as the expected directory identity.
    Healthy,
    /// One or more trailing components do not exist.
    Missing,
    /// A component cannot be inspected with current process authority.
    Inaccessible,
    /// A broken link or otherwise malformed component prevented exact resolution.
    Malformed,
    /// The selected entry exists but is not a directory.
    NotDirectory,
    /// Current symlink resolution differs from the immutable recorded identity.
    IdentityChanged,
    /// Observation failed without a more precise safe classification.
    Unknown,
}

impl PathCondition {
    pub(crate) const fn health(self) -> ResourceHealth {
        match self {
            Self::Healthy => ResourceHealth::Healthy,
            Self::Malformed | Self::IdentityChanged => ResourceHealth::Degraded,
            Self::Missing | Self::Inaccessible | Self::NotDirectory => ResourceHealth::Unavailable,
            Self::Unknown => ResourceHealth::Unknown,
        }
    }

    const fn priority(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Missing => 1,
            Self::Inaccessible => 2,
            Self::Malformed | Self::NotDirectory => 3,
            Self::Unknown => 4,
            Self::IdentityChanged => 5,
        }
    }

    const fn merge(self, other: Self) -> Self {
        if other.priority() > self.priority() {
            other
        } else {
            self
        }
    }
}

/// Passive path identity request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathIdentityRequest {
    /// Immutable home whose local path namespace qualifies the locator.
    pub home: InstallationId,
    /// Stable resource identity assigned by the project mutation.
    pub resource_id: ResourceId,
    /// Absolute spelling selected by the human.
    pub display_path: PathBuf,
}

/// Identified project resource plus its initial exact condition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathResourceResolution {
    /// Home-local namespace qualifying the locators.
    pub home: InstallationId,
    /// Durable display and canonical locators suitable for a project fact.
    pub resource: ProjectResource,
    /// Exact adapter condition behind the coarse semantic health.
    pub condition: PathCondition,
}

/// Closed identity input or root-observation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathResourceError {
    /// Only absolute path resources are supported.
    NotAbsolute,
    /// The platform path cannot be represented by the UTF-8 fact protocol.
    UnsupportedEncoding,
    /// The path contains a byte that cannot be represented safely in the protocol.
    InvalidPath,
    /// The normalized locator exceeds the domain bound.
    LocatorTooLong,
    /// No stable existing ancestor could be observed.
    RootUnavailable,
}

impl std::fmt::Display for PathResourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotAbsolute => "path resource is not absolute",
            Self::UnsupportedEncoding => "path resource encoding is unsupported",
            Self::InvalidPath => "path resource contains an unsupported character",
            Self::LocatorTooLong => "path resource exceeds its bound",
            Self::RootUnavailable => "path resource root is unavailable",
        })
    }
}

impl std::error::Error for PathResourceError {}

pub(crate) fn resolve_path<F: PathSystem>(
    filesystem: &F,
    request: &PathIdentityRequest,
) -> Result<PathResourceResolution, PathResourceError> {
    let display = normalize_absolute_path(&request.display_path)?;
    let mut ancestor = display.clone();
    let mut suffix = Vec::<OsString>::new();
    let mut condition = PathCondition::Healthy;

    let canonical = loop {
        match filesystem.symlink_entry(&ancestor) {
            Ok(_) => match filesystem.canonicalize(&ancestor) {
                Ok(mut observed) => {
                    for component in suffix.iter().rev() {
                        observed.push(component);
                    }
                    let observed = normalize_absolute_path(&observed)?;
                    if suffix.is_empty() {
                        condition = match filesystem.followed_entry(&display) {
                            Ok(PathEntryKind::Directory) => condition,
                            Ok(PathEntryKind::Other | PathEntryKind::SymbolicLink) => {
                                condition.merge(PathCondition::NotDirectory)
                            }
                            Err(error) => condition.merge(condition_from_probe(error)),
                        };
                    } else if condition == PathCondition::Healthy {
                        condition = PathCondition::Missing;
                    }
                    break observed;
                }
                Err(error) => {
                    let observed = match error {
                        PathProbeError::Missing => PathCondition::Malformed,
                        PathProbeError::Inaccessible => PathCondition::Inaccessible,
                        PathProbeError::Unknown => PathCondition::Unknown,
                    };
                    condition = condition.merge(observed);
                }
            },
            Err(error) => condition = condition.merge(condition_from_probe(error)),
        }
        let Some(component) = ancestor.file_name().map(ToOwned::to_owned) else {
            return Err(PathResourceError::RootUnavailable);
        };
        suffix.push(component);
        if !ancestor.pop() {
            return Err(PathResourceError::RootUnavailable);
        }
    };

    let display_locator = working_tree_locator(&display)?;
    let canonical_locator = working_tree_locator(&canonical)?;
    Ok(PathResourceResolution {
        home: request.home,
        resource: ProjectResource {
            resource_id: request.resource_id,
            display_locator,
            canonical_locator,
            health: condition.health(),
        },
        condition,
    })
}

pub(crate) fn working_tree_locator(path: &Path) -> Result<ResourceLocator, PathResourceError> {
    locator(ResourceScheme::WorkingTree, path)
}

pub(crate) fn git_locator(path: &Path) -> Result<ResourceLocator, PathResourceError> {
    locator(ResourceScheme::GitRepository, path)
}

fn locator(scheme: ResourceScheme, path: &Path) -> Result<ResourceLocator, PathResourceError> {
    let value = path
        .to_str()
        .ok_or(PathResourceError::UnsupportedEncoding)?;
    if value.len() > RESOURCE_LOCATOR_MAX_BYTES {
        return Err(PathResourceError::LocatorTooLong);
    }
    let value =
        BoundedText::new(value.to_owned()).map_err(|_| PathResourceError::LocatorTooLong)?;
    Ok(ResourceLocator::new(scheme, value))
}

/// Lexically normalizes one absolute path without observing the filesystem.
///
/// Callers that persist path spellings can compare the result with their input to require one
/// exact reservation identity before performing an external effect.
pub fn normalize_absolute_path(path: &Path) -> Result<PathBuf, PathResourceError> {
    if !path.is_absolute() {
        return Err(PathResourceError::NotAbsolute);
    }
    let value = path
        .to_str()
        .ok_or(PathResourceError::UnsupportedEncoding)?;
    if value.contains('\0') {
        return Err(PathResourceError::InvalidPath);
    }
    if value.len() > RESOURCE_LOCATOR_MAX_BYTES {
        return Err(PathResourceError::LocatorTooLong);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
        }
    }
    Ok(normalized)
}

const fn condition_from_probe(error: PathProbeError) -> PathCondition {
    match error {
        PathProbeError::Missing => PathCondition::Missing,
        PathProbeError::Inaccessible => PathCondition::Inaccessible,
        PathProbeError::Unknown => PathCondition::Unknown,
    }
}
