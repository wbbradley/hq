//! Bounded pairing-artifact filesystem adapter.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use hq_protocol::MAX_PAIRING_INVITATION_BYTES;

/// Closed pairing file failure without caller-controlled path detail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PairingFileError;

/// Creates and durably writes one bounded absolute regular file without overwrite.
pub(crate) fn write_new_pairing_file(path: &Path, bytes: &[u8]) -> Result<(), PairingFileError> {
    if !path.is_absolute() || bytes.len() > MAX_PAIRING_INVITATION_BYTES {
        return Err(PairingFileError);
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(PairingFileError),
    }
    let parent = path.parent().ok_or(PairingFileError)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|_| PairingFileError)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(PairingFileError);
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|_| PairingFileError)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| PairingFileError)?;
    let mut directory_options = OpenOptions::new();
    directory_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        directory_options.custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW);
    }
    directory_options
        .open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| PairingFileError)
}

/// Reads one bounded absolute existing regular non-symlink file.
pub(crate) fn read_pairing_file(path: &Path) -> Result<Vec<u8>, PairingFileError> {
    if !path.is_absolute() {
        return Err(PairingFileError);
    }
    let maximum = u64::try_from(MAX_PAIRING_INVITATION_BYTES).map_err(|_| PairingFileError)?;
    let mut bytes = Vec::new();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| PairingFileError)?;
    let metadata = file.metadata().map_err(|_| PairingFileError)?;
    if !metadata.is_file() || metadata.len() > MAX_PAIRING_INVITATION_BYTES as u64 {
        return Err(PairingFileError);
    }
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| PairingFileError)?;
    if bytes.len() > MAX_PAIRING_INVITATION_BYTES {
        return Err(PairingFileError);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn pairing_files_are_bounded_new_absolute_regular_files() {
        let directory =
            std::env::temp_dir().join(format!("hq-cli-pairing-file-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).expect("test directory");
        let invitation = directory.join("invite.json");
        write_new_pairing_file(&invitation, b"pairing").expect("new file writes");
        assert_eq!(read_pairing_file(&invitation), Ok(b"pairing".to_vec()));
        assert_eq!(
            write_new_pairing_file(&invitation, b"replacement"),
            Err(PairingFileError)
        );

        let symlink = directory.join("link.json");
        std::os::unix::fs::symlink(&invitation, &symlink).expect("test symlink");
        assert_eq!(read_pairing_file(&symlink), Err(PairingFileError));
        fs::remove_dir_all(&directory).expect("test directory removes");
    }
}
