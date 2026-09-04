//! Explicit state layout and exclusive local ownership.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{IdentityError, IdentityErrorClass};

/// Stable paths owned by one Rust installation state directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePaths {
    root: PathBuf,
    identity: PathBuf,
    configuration: PathBuf,
    database: PathBuf,
    ownership: PathBuf,
}

impl StatePaths {
    /// Constructs the layout below one explicit absolute state root.
    pub fn new(root: PathBuf) -> Result<Self, IdentityError> {
        if !root.is_absolute() || root.as_os_str().is_empty() {
            return Err(IdentityError::new(IdentityErrorClass::InvalidPath));
        }
        Ok(Self {
            identity: root.join("identity.v1"),
            configuration: root.join("local-config.v1.json"),
            database: root.join("hq.sqlite3"),
            ownership: root.join("node.lock"),
            root,
        })
    }

    /// Derives the default root from XDG state and home directory inputs.
    pub fn derive(
        xdg_state_home: Option<&Path>,
        home: Option<&Path>,
    ) -> Result<Self, IdentityError> {
        let root = if let Some(xdg) = xdg_state_home {
            if !xdg.is_absolute() || xdg.as_os_str().is_empty() {
                return Err(IdentityError::new(IdentityErrorClass::InvalidPath));
            }
            xdg.join("hq")
        } else if let Some(home) = home {
            if !home.is_absolute() || home.as_os_str().is_empty() {
                return Err(IdentityError::new(IdentityErrorClass::InvalidPath));
            }
            home.join(".local/state/hq")
        } else {
            return Err(IdentityError::new(IdentityErrorClass::PathUnavailable));
        };
        Self::new(root)
    }

    /// Derives paths from `XDG_STATE_HOME` and `HOME`.
    pub fn from_environment() -> Result<Self, IdentityError> {
        let xdg = std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self::derive(xdg.as_deref(), home.as_deref())
    }

    /// Returns the state root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the fixed identity-file path.
    pub fn identity_file(&self) -> &Path {
        &self.identity
    }

    /// Returns the unsigned local configuration path.
    pub fn configuration_file(&self) -> &Path {
        &self.configuration
    }

    /// Returns the future Rust SQLite path.
    pub fn database_file(&self) -> &Path {
        &self.database
    }

    /// Returns the local ownership-lock path.
    pub fn ownership_file(&self) -> &Path {
        &self.ownership
    }
}

/// Exclusive same-state owner retained for the complete node lifetime.
pub struct StateDirectoryOwner {
    pub(super) paths: StatePaths,
    pub(super) ownership: Arc<File>,
}

impl StateDirectoryOwner {
    /// Creates or validates the private state directory and acquires its exclusive lock.
    pub fn acquire(paths: StatePaths) -> Result<Self, IdentityError> {
        ensure_private_directory(paths.root())?;
        reject_symlink(paths.ownership_file())?;
        let ownership = private_options()
            .read(true)
            .write(true)
            .create(true)
            .open(paths.ownership_file())
            .map_err(file_system)?;
        ensure_private_file(&ownership)?;
        match ownership.try_lock() {
            Ok(()) => Ok(Self {
                paths,
                ownership: Arc::new(ownership),
            }),
            Err(TryLockError::WouldBlock) => {
                Err(IdentityError::new(IdentityErrorClass::AlreadyOwned))
            }
            Err(TryLockError::Error(_)) => Err(IdentityError::new(IdentityErrorClass::FileSystem)),
        }
    }

    /// Returns the exact layout held by this owner.
    pub const fn paths(&self) -> &StatePaths {
        &self.paths
    }
}

impl std::fmt::Debug for StateDirectoryOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StateDirectoryOwner")
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

pub(super) fn reject_symlink(path: &Path) -> Result<(), IdentityError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(IdentityError::new(IdentityErrorClass::SymbolicLink))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(IdentityError::new(IdentityErrorClass::FileSystem)),
    }
}

pub(super) fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub(super) fn ensure_private_file(file: &File) -> Result<(), IdentityError> {
    let metadata = file.metadata().map_err(file_system)?;
    if !metadata.is_file() {
        return Err(IdentityError::new(IdentityErrorClass::FileSystem));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(IdentityError::new(IdentityErrorClass::UnsafePermissions));
        }
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), IdentityError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(IdentityError::new(IdentityErrorClass::SymbolicLink));
            }
            if !metadata.is_dir() {
                return Err(IdentityError::new(IdentityErrorClass::FileSystem));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o777 != 0o700 {
                    return Err(IdentityError::new(IdentityErrorClass::UnsafePermissions));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(file_system)?;
            #[cfg(unix)]
            fs::set_permissions(path, unix_permissions(0o700)).map_err(file_system)?;
            sync_directory(path)
        }
        Err(_) => Err(IdentityError::new(IdentityErrorClass::FileSystem)),
    }
}

pub(super) fn sync_directory(path: &Path) -> Result<(), IdentityError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(file_system)
}

#[cfg(unix)]
fn unix_permissions(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}

pub(super) fn file_system(_: std::io::Error) -> IdentityError {
    IdentityError::new(IdentityErrorClass::FileSystem)
}
