#![allow(clippy::expect_used, clippy::panic, dead_code)]

use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use hq_protocol::{Bip340Signer, DispatchOutcome, VerifiedSemanticFact};
use hq_store::Store;

pub trait TestStoreExt {
    fn append_verified(
        &self,
        fact: VerifiedSemanticFact,
    ) -> Result<hq_store::IngestOutcome, hq_store::StoreError>;
}

impl TestStoreExt for Store {
    fn append_verified(
        &self,
        fact: VerifiedSemanticFact,
    ) -> Result<hq_store::IngestOutcome, hq_store::StoreError> {
        self.ingest_verified(fact, authority_policy())
    }
}

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

pub fn verified_fact_with_label(
    label: &str,
    auxiliary_randomness: [u8; 32],
) -> VerifiedSemanticFact {
    let content =
        CANONICAL_CONTENT.replace("\"label\":\"alpha\"", &format!("\"label\":\"{label}\""));
    signed_fact(0, content.as_bytes(), auxiliary_randomness)
}

pub fn authority_policy() -> hq_reducer::AuthorityPolicy {
    hq_reducer::AuthorityPolicy::new(
        hq_domain::InstallationId::from_bytes([0x11; 32]),
        hq_domain::MailboxId::from_bytes([0x33; 32]),
    )
}

pub fn verified_fact_with_auxiliary(auxiliary_randomness: [u8; 32]) -> VerifiedSemanticFact {
    signed_fact(0, CANONICAL_CONTENT.as_bytes(), auxiliary_randomness)
}

fn signed_fact(
    created_at: u64,
    content: &[u8],
    auxiliary_randomness: [u8; 32],
) -> VerifiedSemanticFact {
    let signer = Bip340Signer::from_secret_bytes({
        let mut secret = [0_u8; 32];
        secret[31] = 1;
        secret
    })
    .expect("fixture secret is valid");
    let event = signer
        .sign(created_at, content, auxiliary_randomness)
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

pub fn verified_device_grant(account_fact_id: [u8; 32]) -> VerifiedSemanticFact {
    let account = hex(&account_fact_id);
    let device_signing = hex(&signer(2).public_key());
    let content = format!(
        r#"{{"p":"hq/canonical","v":1,"f":12,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":5000,"scope":["account","5555555555555555555555555555555555555555555555555555555555555555"],"parents":[["c","{account}"]],"auth":[["account-creator","c","{account}"]],"body":{{"account":"5555555555555555555555555555555555555555555555555555555555555555","grant":"8888888888888888888888888888888888888888888888888888888888888888","device":{{"installation":"2222222222222222222222222222222222222222222222222222222222222222","signing":"{device_signing}"}},"label":"device-two","relays":[]}}}}"#
    );
    signed_fact(5, content.as_bytes(), [14; 32])
}

pub fn verified_device_acceptance(grant_fact_id: [u8; 32]) -> VerifiedSemanticFact {
    let grant = hex(&grant_fact_id);
    let device = signer(2);
    let device_signing = hex(&device.public_key());
    let content = format!(
        r#"{{"p":"hq/canonical","v":1,"f":13,"author":"2222222222222222222222222222222222222222222222222222222222222222","time":6000,"scope":["account","5555555555555555555555555555555555555555555555555555555555555555"],"parents":[["c","{grant}"]],"auth":[["device-grant","c","{grant}"]],"body":{{"account":"5555555555555555555555555555555555555555555555555555555555555555","grant":"8888888888888888888888888888888888888888888888888888888888888888","device":{{"installation":"2222222222222222222222222222222222222222222222222222222222222222","signing":"{device_signing}"}}}}}}"#
    );
    let event = device
        .sign(6, content.as_bytes(), [15; 32])
        .expect("device acceptance signs");
    let DispatchOutcome::Supported(supported) = event.dispatch().expect("acceptance dispatches")
    else {
        panic!("acceptance protocol is supported");
    };
    supported
        .decode_v1()
        .expect("acceptance DTO verifies")
        .into_semantic_fact()
        .expect("acceptance converts")
}

fn signer(secret_value: u8) -> Bip340Signer {
    Bip340Signer::from_secret_bytes({
        let mut secret = [0_u8; 32];
        secret[31] = secret_value;
        secret
    })
    .expect("fixture secret is valid")
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

pub fn verified_session_binding(
    installation_fact_id: [u8; 32],
    mailbox_fact_id: [u8; 32],
) -> VerifiedSemanticFact {
    let installation = hex(&installation_fact_id);
    let mailbox = hex(&mailbox_fact_id);
    let mut parents = [installation.as_str(), mailbox.as_str()];
    parents.sort_unstable();
    let content = format!(
        r#"{{"p":"hq/canonical","v":1,"f":3,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":2000,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[["c","{}"],["c","{}"]],"auth":[["local-installation","c","{installation}"]],"body":{{"mailbox":"3333333333333333333333333333333333333333333333333333333333333333","provider":"test-provider","session":"session-1"}}}}"#,
        parents[0], parents[1]
    );
    signed_fact(2, content.as_bytes(), [11; 32])
}

pub fn verified_account(installation_fact_id: [u8; 32]) -> VerifiedSemanticFact {
    let installation = hex(&installation_fact_id);
    let content = format!(
        r#"{{"p":"hq/canonical","v":1,"f":10,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":3000,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[["c","{installation}"]],"auth":[["local-installation","c","{installation}"]],"body":{{"account":"5555555555555555555555555555555555555555555555555555555555555555","creator":{{"installation":"1111111111111111111111111111111111111111111111111111111111111111","signing":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"}},"label":"primary"}}}}"#
    );
    signed_fact(3, content.as_bytes(), [12; 32])
}

pub fn verified_project(
    installation_fact_id: [u8; 32],
    account_fact_id: [u8; 32],
) -> VerifiedSemanticFact {
    let installation = hex(&installation_fact_id);
    let account = hex(&account_fact_id);
    let mut parents = [installation.as_str(), account.as_str()];
    parents.sort_unstable();
    let content = format!(
        r#"{{"p":"hq/canonical","v":1,"f":27,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":4000,"scope":["account","5555555555555555555555555555555555555555555555555555555555555555"],"parents":[["c","{}"],["c","{}"]],"auth":[["account-membership","c","{account}"],["active-human","c","{account}"],["project-home","c","{installation}"]],"body":{{"project":"6666666666666666666666666666666666666666666666666666666666666666","mailbox":"4444444444444444444444444444444444444444444444444444444444444444","home":"1111111111111111111111111111111111111111111111111111111111111111","name":"project-one","brief":"signed project","predecessor":null,"resources":[{{"id":"7777777777777777777777777777777777777777777777777777777777777777","locator":{{"scheme":"worktree","value":"/workspace/project"}},"health":"healthy"}}],"primary":"7777777777777777777777777777777777777777777777777777777777777777","state":"open"}}}}"#,
        parents[0], parents[1]
    );
    signed_fact(4, content.as_bytes(), [13; 32])
}

pub fn verified_question(parent_id: [u8; 32]) -> VerifiedSemanticFact {
    let parent = hex(&parent_id);
    let content = format!(
        r#"{{"p":"hq/canonical","v":1,"f":15,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":2000,"scope":["local","1111111111111111111111111111111111111111111111111111111111111111"],"parents":[["c","{parent}"]],"auth":[["local-installation","c","{parent}"]],"body":{{"id":"5555555555555555555555555555555555555555555555555555555555555555","sender":{{"installation":"1111111111111111111111111111111111111111111111111111111111111111","mailbox":"3333333333333333333333333333333333333333333333333333333333333333"}},"recipient":{{"installation":"1111111111111111111111111111111111111111111111111111111111111111","mailbox":"3333333333333333333333333333333333333333333333333333333333333333"}},"body":"question","purpose":"question","presentation":"message","correlation":{{"provider":"test-provider","session":"session-1","id":"7777777777777777777777777777777777777777777777777777777777777777"}},"project":null}}}}"#
    );
    signed_fact(2, content.as_bytes(), [10; 32])
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
