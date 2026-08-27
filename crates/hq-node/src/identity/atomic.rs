//! Same-directory private durable atomic writes.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use super::{
    IdentityError, IdentityErrorClass,
    paths::{ensure_private_file, file_system, private_options, sync_directory},
};

#[derive(Clone, Copy)]
pub(super) enum WriteMode {
    CreateNew(IdentityErrorClass),
    Replace,
}

pub(super) fn atomic_write(
    target: &Path,
    bytes: &[u8],
    mode: WriteMode,
) -> Result<(), IdentityError> {
    let parent = target
        .parent()
        .ok_or_else(|| IdentityError::new(IdentityErrorClass::InvalidPath))?;
    let mut suffix = [0_u8; 16];
    getrandom::fill(&mut suffix)
        .map_err(|_| IdentityError::new(IdentityErrorClass::EntropyUnavailable))?;
    let temporary = temporary_path(target, suffix);
    let guard = TemporaryFile::new(temporary.clone());
    let mut file = private_options()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(file_system)?;
    ensure_private_file(&file)?;
    file.write_all(bytes).map_err(file_system)?;
    file.sync_all().map_err(file_system)?;
    drop(file);

    match mode {
        WriteMode::CreateNew(exists) => {
            fs::hard_link(&temporary, target).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    IdentityError::new(exists)
                } else {
                    file_system(error)
                }
            })?;
            guard.remove()?;
        }
        WriteMode::Replace => {
            fs::rename(&temporary, target).map_err(file_system)?;
            guard.forget();
        }
    }
    sync_directory(parent)
}

fn temporary_path(target: &Path, suffix: [u8; 16]) -> PathBuf {
    let mut name = target.file_name().map_or_else(
        || "hq".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    name.push_str(".tmp-");
    append_hex(&mut name, &suffix);
    target.with_file_name(name)
}

fn append_hex(output: &mut String, bytes: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
}

struct TemporaryFile {
    path: Option<PathBuf>,
}

impl TemporaryFile {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn remove(mut self) -> Result<(), IdentityError> {
        let path = self
            .path
            .take()
            .ok_or_else(|| IdentityError::new(IdentityErrorClass::FileSystem))?;
        fs::remove_file(path).map_err(file_system)
    }

    fn forget(mut self) {
        self.path = None;
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}
