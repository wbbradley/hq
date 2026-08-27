#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub struct TestDirectory(PathBuf);

impl TestDirectory {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("hq-{}-{unique}", std::process::id()));
        fs::create_dir(&path).expect("test directory creates");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("fixture writes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("fixture mode sets");
    }
}

#[cfg(unix)]
pub fn assert_private_mode(path: &Path, expected: u32) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .expect("metadata exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, expected, "unexpected mode for {}", path.display());
}

#[cfg(not(unix))]
pub fn assert_private_mode(_: &Path, _: u32) {}
