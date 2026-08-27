#![allow(clippy::expect_used, clippy::panic, dead_code)]

use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use hq_protocol::{Bip340Signer, DispatchOutcome, VerifiedSemanticFact};
use hq_store::Store;

pub const CANONICAL_CONTENT: &str = r#"{"p":"hq/canonical","v":1,"f":1,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":0,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[],"auth":[],"body":{"installation":"1111111111111111111111111111111111111111111111111111111111111111","signing":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","encryption":"2222222222222222222222222222222222222222222222222222222222222222","label":"alpha"}}"#;

pub struct TestDirectory(PathBuf);

impl TestDirectory {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hq-rust-store-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("test directory creates");
        Self(path)
    }

    pub fn database_path(&self) -> PathBuf {
        self.0.join("state").join("hq.sqlite3")
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

pub fn open_store(path: &Path) -> Store {
    Store::open(path, NonZeroUsize::new(4).expect("nonzero capacity")).expect("store opens")
}

pub fn verified_fact() -> VerifiedSemanticFact {
    verified_fact_with_auxiliary([7; 32])
}

pub fn verified_fact_with_auxiliary(auxiliary_randomness: [u8; 32]) -> VerifiedSemanticFact {
    let signer = Bip340Signer::from_secret_bytes({
        let mut secret = [0_u8; 32];
        secret[31] = 1;
        secret
    })
    .expect("fixture secret is valid");
    let event = signer
        .sign(0, CANONICAL_CONTENT.as_bytes(), auxiliary_randomness)
        .expect("fixture signs");
    let DispatchOutcome::Supported(supported) = event.dispatch().expect("fixture dispatches")
    else {
        panic!("fixture protocol is supported");
    };
    supported
        .decode_v1()
        .expect("fixture DTO verifies")
        .into_semantic_fact()
        .expect("fixture converts")
}

pub fn verified_child(parent_id: [u8; 32]) -> VerifiedSemanticFact {
    let parent = hex(&parent_id);
    let content = format!(
        r#"{{"p":"hq/canonical","v":1,"f":2,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":1000,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[["c","{parent}"]],"auth":[["local-installation","c","{parent}"]],"body":{{"mailbox":"3333333333333333333333333333333333333333333333333333333333333333","kind":"agent","label":"helper"}}}}"#
    );
    let signer = Bip340Signer::from_secret_bytes({
        let mut secret = [0_u8; 32];
        secret[31] = 1;
        secret
    })
    .expect("fixture secret is valid");
    let event = signer
        .sign(1, content.as_bytes(), [9; 32])
        .expect("child fixture signs");
    let DispatchOutcome::Supported(supported) = event.dispatch().expect("child dispatches") else {
        panic!("child protocol is supported");
    };
    supported
        .decode_v1()
        .expect("child DTO verifies")
        .into_semantic_fact()
        .expect("child converts")
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(unix)]
pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .expect("metadata exists")
        .permissions()
        .mode()
        & 0o777
}
