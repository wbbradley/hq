//! Secure first-release Unix runtime namespace.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use hq_domain::InstallationId;

/// Maximum pathname bytes including every directory but excluding the terminating NUL.
///
/// macOS exposes 104 bytes for `sockaddr_un.sun_path`; reserving one byte for NUL makes this the
/// portable limit across the first-release Linux and macOS targets.
pub const PORTABLE_UNIX_SOCKET_PATH_BYTES: usize = 103;

/// Stable runtime-path validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimePathErrorClass {
    /// The root was empty or relative.
    InvalidPath,
    /// The final socket pathname exceeds the supported Linux/macOS limit.
    SocketPathTooLong,
    /// The runtime root or a reserved artifact is a symbolic link.
    SymbolicLink,
    /// An existing runtime root is accessible beyond its owner.
    UnsafePermissions,
    /// A filesystem operation failed without exposing platform prose.
    FileSystem,
}

/// Redacted runtime-path failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimePathError {
    class: RuntimePathErrorClass,
}

impl RuntimePathError {
    const fn new(class: RuntimePathErrorClass) -> Self {
        Self { class }
    }

    /// Returns the stable failure classification.
    pub const fn class(self) -> RuntimePathErrorClass {
        self.class
    }
}

impl fmt::Display for RuntimePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.class {
            RuntimePathErrorClass::InvalidPath => "runtime path is invalid",
            RuntimePathErrorClass::SocketPathTooLong => "runtime socket path is too long",
            RuntimePathErrorClass::SymbolicLink => "runtime artifact must not be a symbolic link",
            RuntimePathErrorClass::UnsafePermissions => "runtime permissions are unsafe",
            RuntimePathErrorClass::FileSystem => "runtime filesystem operation failed",
        })
    }
}

impl Error for RuntimePathError {}

/// Stable runtime artifacts owned by one installation's node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimePaths {
    root: PathBuf,
    socket: PathBuf,
    readiness: PathBuf,
}

impl RuntimePaths {
    /// Constructs paths below one explicit absolute runtime root.
    pub fn new(root: PathBuf) -> Result<Self, RuntimePathError> {
        if !root.is_absolute() || root.as_os_str().is_empty() {
            return Err(RuntimePathError::new(RuntimePathErrorClass::InvalidPath));
        }
        let socket = root.join("node.sock");
        if socket.as_os_str().as_encoded_bytes().len() > PORTABLE_UNIX_SOCKET_PATH_BYTES {
            return Err(RuntimePathError::new(
                RuntimePathErrorClass::SocketPathTooLong,
            ));
        }
        Ok(Self {
            readiness: root.join("node-ready.v1.json"),
            root,
            socket,
        })
    }

    /// Derives an installation-qualified XDG root or a private state-local fallback.
    pub fn derive(
        xdg_runtime_directory: Option<&Path>,
        state_root: &Path,
        installation: InstallationId,
    ) -> Result<Self, RuntimePathError> {
        if !state_root.is_absolute() || state_root.as_os_str().is_empty() {
            return Err(RuntimePathError::new(RuntimePathErrorClass::InvalidPath));
        }
        let root = if let Some(xdg) = xdg_runtime_directory {
            if !xdg.is_absolute() || xdg.as_os_str().is_empty() {
                return Err(RuntimePathError::new(RuntimePathErrorClass::InvalidPath));
            }
            xdg.join("hq")
                .join(encode_hex(&installation.as_bytes()[..12]))
        } else {
            state_root.join("runtime")
        };
        Self::new(root)
    }

    /// Returns the private runtime root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the reserved Unix socket path.
    pub fn socket_file(&self) -> &Path {
        &self.socket
    }

    /// Returns the reserved atomic readiness-metadata path.
    pub fn readiness_file(&self) -> &Path {
        &self.readiness
    }
}

/// Validated runtime directory retained for the node lifetime.
#[derive(Debug)]
pub struct RuntimeDirectoryOwner {
    paths: RuntimePaths,
}

impl RuntimeDirectoryOwner {
    /// Creates or validates the private root without deleting any existing runtime artifact.
    pub fn prepare(paths: RuntimePaths) -> Result<Self, RuntimePathError> {
        ensure_private_directory(paths.root())?;
        reject_symlink(paths.socket_file())?;
        reject_symlink(paths.readiness_file())?;
        Ok(Self { paths })
    }

    /// Returns the exact validated runtime layout.
    pub const fn paths(&self) -> &RuntimePaths {
        &self.paths
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), RuntimePathError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(RuntimePathError::new(RuntimePathErrorClass::SymbolicLink));
            }
            if !metadata.is_dir() {
                return Err(RuntimePathError::new(RuntimePathErrorClass::FileSystem));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o777 != 0o700 {
                    return Err(RuntimePathError::new(
                        RuntimePathErrorClass::UnsafePermissions,
                    ));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(file_system)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(file_system)?;
            }
            fs::File::open(path)
                .and_then(|directory| directory.sync_all())
                .map_err(file_system)
        }
        Err(_) => Err(RuntimePathError::new(RuntimePathErrorClass::FileSystem)),
    }
}

fn reject_symlink(path: &Path) -> Result<(), RuntimePathError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(RuntimePathError::new(RuntimePathErrorClass::SymbolicLink))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RuntimePathError::new(RuntimePathErrorClass::FileSystem)),
    }
}

fn file_system(_: std::io::Error) -> RuntimePathError {
    RuntimePathError::new(RuntimePathErrorClass::FileSystem)
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}
