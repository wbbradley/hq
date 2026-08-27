#![allow(clippy::expect_used, dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

pub struct TestDirectory(PathBuf);

impl TestDirectory {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hq-resources-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("test directory creates");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, value: &str) -> PathBuf {
        self.0.join(value)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .expect("git starts");
    assert!(
        output.status.success(),
        "git {:?} failed with {:?}",
        arguments,
        output.status.code()
    );
}

pub fn initialize_repository(directory: &Path) {
    git(directory, &["init", "-q"]);
    git(directory, &["config", "user.email", "hq@example.invalid"]);
    git(directory, &["config", "user.name", "HQ Test"]);
    fs::write(directory.join("tracked.txt"), b"initial\n").expect("tracked file writes");
    git(directory, &["add", "tracked.txt"]);
    git(directory, &["commit", "-qm", "initial"]);
}
