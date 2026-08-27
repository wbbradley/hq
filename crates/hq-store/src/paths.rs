//! Private database path creation and validation.

use std::{ffi::OsString, fs, fs::OpenOptions, path::Path};

use crate::{StoreError, StoreErrorClass};

pub(super) fn prepare_database_path(path: &Path) -> Result<bool, StoreError> {
    if !path.is_absolute() || path.as_os_str().is_empty() {
        return Err(StoreError::new(StoreErrorClass::InvalidPath));
    }
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .ok_or_else(|| StoreError::new(StoreErrorClass::InvalidPath))?;
    ensure_private_directory(parent)?;
    reject_symlink(path)?;
    validate_sidecars(path)?;

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(StoreError::new(StoreErrorClass::InvalidPath));
            }
            ensure_mode(&metadata, 0o600)?;
            Ok(metadata.len() == 0)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options
                .open(path)
                .map_err(|_| StoreError::new(StoreErrorClass::FileSystem))?;
            file.sync_all()
                .map_err(|_| StoreError::new(StoreErrorClass::FileSystem))?;
            ensure_mode(
                &file
                    .metadata()
                    .map_err(|_| StoreError::new(StoreErrorClass::FileSystem))?,
                0o600,
            )?;
            sync_directory(parent)?;
            Ok(true)
        }
        Err(_) => Err(StoreError::new(StoreErrorClass::FileSystem)),
    }
}

pub(super) fn validate_database_path(path: &Path) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .ok_or_else(|| StoreError::new(StoreErrorClass::InvalidPath))?;
    ensure_private_directory(parent)?;
    reject_symlink(path)?;
    validate_sidecars(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| StoreError::new(StoreErrorClass::FileSystem))?;
    if !metadata.is_file() {
        return Err(StoreError::new(StoreErrorClass::InvalidPath));
    }
    ensure_mode(&metadata, 0o600)
}

pub(super) fn validate_sidecars(path: &Path) -> Result<(), StoreError> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = OsString::from(path.as_os_str());
        sidecar.push(suffix);
        let sidecar = Path::new(&sidecar);
        match fs::symlink_metadata(sidecar) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(StoreError::new(StoreErrorClass::SymbolicLink));
                }
                if !metadata.is_file() {
                    return Err(StoreError::new(StoreErrorClass::InvalidPath));
                }
                ensure_mode(&metadata, 0o600)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(StoreError::new(StoreErrorClass::FileSystem)),
        }
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(StoreError::new(StoreErrorClass::SymbolicLink));
            }
            if !metadata.is_dir() {
                return Err(StoreError::new(StoreErrorClass::InvalidPath));
            }
            ensure_mode(&metadata, 0o700)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let ancestor = path
                .parent()
                .filter(|value| !value.as_os_str().is_empty())
                .ok_or_else(|| StoreError::new(StoreErrorClass::InvalidPath))?;
            if let Ok(metadata) = fs::symlink_metadata(ancestor)
                && metadata.file_type().is_symlink()
            {
                return Err(StoreError::new(StoreErrorClass::SymbolicLink));
            }
            fs::create_dir_all(path).map_err(|_| StoreError::new(StoreErrorClass::FileSystem))?;
            #[cfg(unix)]
            fs::set_permissions(path, unix_permissions(0o700))
                .map_err(|_| StoreError::new(StoreErrorClass::FileSystem))?;
            sync_directory(path)
        }
        Err(_) => Err(StoreError::new(StoreErrorClass::FileSystem)),
    }
}

fn reject_symlink(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(StoreError::new(StoreErrorClass::SymbolicLink))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StoreError::new(StoreErrorClass::FileSystem)),
    }
}

fn ensure_mode(metadata: &fs::Metadata, expected: u32) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != expected {
            return Err(StoreError::new(StoreErrorClass::UnsafePermissions));
        }
    }
    #[cfg(not(unix))]
    let _ = (metadata, expected);
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| StoreError::new(StoreErrorClass::FileSystem))
}

#[cfg(unix)]
fn unix_permissions(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}
