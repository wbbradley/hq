#![allow(clippy::expect_used, clippy::panic, dead_code)]

use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use hq_domain::{
    ActivityKind, ActivityStatus, AuthorityReference, AuthorityRole, BoundedSet, CausalReferences,
    ContentText, FactId, FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS,
    MailboxAddress, MessageContent, MessageId, MessagePurpose, OperationCorrelation, OperationId,
    PresentationKind, ProjectId, ProviderId, ProviderSessionId, SemanticPayload, ShortText,
    Timestamp,
};
use hq_protocol::{Bip340Signer, CanonicalEventPlan, DispatchOutcome, VerifiedSemanticFact};
use hq_store::Store;
use rusqlite::{Connection, params};

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

pub fn seed_canonical_corpus(path: &Path, facts: &[VerifiedSemanticFact]) {
    let mut connection = Connection::open(path).expect("seed connection opens");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("foreign keys enable");
    let transaction = connection.transaction().expect("seed transaction begins");
    for verified in facts {
        let fact = verified.fact();
        let family = fact
            .kind()
            .catalog_id()
            .strip_prefix("FCT-")
            .expect("catalog prefix exists")
            .parse::<i64>()
            .expect("catalog suffix is numeric");
        transaction
            .execute(
                "INSERT INTO canonical_facts(fact_id, event_bytes, namespace, family) \
                 VALUES (?1, ?2, 1, ?3)",
                params![
                    fact.id().as_bytes().as_slice(),
                    verified.verified_event().exact_event_bytes(),
                    family
                ],
            )
            .expect("canonical fact seeds");
        for parent in fact.causal().parents().iter() {
            transaction
                .execute(
                    "INSERT INTO fact_parents(fact_id, parent_id) VALUES (?1, ?2)",
                    params![
                        fact.id().as_bytes().as_slice(),
                        parent.as_bytes().as_slice()
                    ],
                )
                .expect("parent seeds");
        }
        for role in AuthorityRole::ALL {
            if let Some(authority) = fact.causal().authority(role) {
                transaction
                    .execute(
                        "INSERT INTO fact_authorities(fact_id, authority_role, authority_fact_id) \
                         VALUES (?1, ?2, ?3)",
                        params![
                            fact.id().as_bytes().as_slice(),
                            authority_role_code(role),
                            authority.as_bytes().as_slice()
                        ],
                    )
                    .expect("authority seeds");
            }
        }
    }
    transaction.commit().expect("seed transaction commits");
}

fn authority_role_code(role: AuthorityRole) -> i64 {
    AuthorityRole::ALL
        .iter()
        .position(|candidate| *candidate == role)
        .and_then(|index| i64::try_from(index + 1).ok())
        .expect("closed role has a code")
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

pub fn signer(secret_value: u8) -> Bip340Signer {
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
        r#"{{"p":"hq/canonical","v":1,"f":27,"author":"1111111111111111111111111111111111111111111111111111111111111111","time":4000,"scope":["account","5555555555555555555555555555555555555555555555555555555555555555"],"parents":[["c","{}"],["c","{}"]],"auth":[["account-membership","c","{account}"],["active-human","c","{account}"],["project-home","c","{installation}"]],"body":{{"project":"6666666666666666666666666666666666666666666666666666666666666666","mailbox":"4444444444444444444444444444444444444444444444444444444444444444","home":"1111111111111111111111111111111111111111111111111111111111111111","name":"project-one","brief":"signed project","predecessor":null,"resources":[{{"id":"7777777777777777777777777777777777777777777777777777777777777777","display":{{"scheme":"worktree","value":"/workspace/project"}},"canonical":{{"scheme":"worktree","value":"/workspace/project"}},"health":"healthy"}}],"primary":"7777777777777777777777777777777777777777777777777777777777777777","state":"open"}}}}"#,
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

pub fn verified_incomplete_peer_question() -> VerifiedSemanticFact {
    let local = authority_policy().local_installation();
    let sender = MailboxAddress::new(local, hq_domain::MailboxId::from_bytes([0x33; 32]));
    let recipient = MailboxAddress::new(
        InstallationId::from_bytes([0x44; 32]),
        hq_domain::MailboxId::from_bytes([0x55; 32]),
    );
    let missing_grant = FactId::from_bytes([0xaa; 32]);
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([missing_grant]).expect("one parent"),
        [AuthorityReference::new(
            AuthorityRole::MailboxGrant,
            missing_grant,
        )],
    )
    .expect("missing grant remains a structural authority edge");
    CanonicalEventPlan::new(
        local,
        Timestamp::from_unix_millis(2_500),
        FactScope::PeerAddressed(recipient),
        causal,
        SemanticPayload::QuestionAsked(MessageContent {
            message_id: MessageId::from_bytes([0x66; 32]),
            sender,
            recipient: Some(recipient),
            body: ContentText::new("incomplete peer question").expect("bounded body"),
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: None,
            project_id: None,
        }),
    )
    .sign(&signer(1), [0x77; 32])
    .expect("incomplete question signs")
}

pub fn authored_conversation_entry(index: u16, activity: bool) -> VerifiedSemanticFact {
    authored_conversation_entry_with_retention(index, activity, false)
}

pub fn authored_durable_conversation_entry(index: u16, activity: bool) -> VerifiedSemanticFact {
    authored_conversation_entry_with_retention(index, activity, true)
}

#[allow(clippy::too_many_arguments)]
pub fn authored_agent_activity(
    index: u16,
    operation: OperationId,
    item: Option<&str>,
    kind: ActivityKind,
    logical_key: &str,
    sequence: u64,
    status: ActivityStatus,
    content: &str,
) -> VerifiedSemanticFact {
    let root = FactId::from_bytes(verified_fact().verified_event().event_id());
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([root]).expect("one parent validates"),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
    )
    .expect("local authority validates");
    let local = MailboxAddress::new(
        authority_policy().local_installation(),
        authority_policy().local_human_mailbox(),
    );
    let mut auxiliary = [0_u8; 32];
    auxiliary[0] = 9;
    auxiliary[30..].copy_from_slice(&index.to_be_bytes());
    CanonicalEventPlan::new(
        authority_policy().local_installation(),
        Timestamp::from_unix_millis(3_000 + i64::from(index)),
        FactScope::InstallationPrivate(authority_policy().local_installation()),
        causal,
        SemanticPayload::HarnessActivityRecorded {
            project: None,
            source: local,
            correlation: OperationCorrelation::new(
                ProviderId::new("paged-provider").expect("provider validates"),
                ProviderSessionId::new("paged-session").expect("session validates"),
                operation,
            ),
            item: item.map(|value| ShortText::new(value).expect("item validates")),
            kind,
            logical_key: ShortText::new(logical_key).expect("logical key validates"),
            runtime: ShortText::new("runtime-1").expect("runtime validates"),
            sequence: std::num::NonZeroU64::new(sequence).expect("sequence is positive"),
            occurred_at: Timestamp::from_unix_millis(3_000 + i64::from(index)),
            status,
            content: ContentText::new(content).expect("content validates"),
            truncated: false,
            completed: (kind == ActivityKind::CompletedItem)
                .then_some(hq_domain::CompletedItemPresentation::Unknown),
        },
    )
    .sign(&signer(1), auxiliary)
    .expect("agent activity signs")
}

pub fn authored_local_message(
    index: u16,
    operation: OperationId,
    body: &str,
) -> VerifiedSemanticFact {
    let root = FactId::from_bytes(verified_fact().verified_event().event_id());
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([root]).expect("one parent validates"),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
    )
    .expect("local authority validates");
    let local = MailboxAddress::new(
        authority_policy().local_installation(),
        authority_policy().local_human_mailbox(),
    );
    let mut auxiliary = [0_u8; 32];
    auxiliary[0] = 10;
    auxiliary[30..].copy_from_slice(&index.to_be_bytes());
    CanonicalEventPlan::new(
        authority_policy().local_installation(),
        Timestamp::from_unix_millis(3_000 + i64::from(index)),
        FactScope::InstallationPrivate(authority_policy().local_installation()),
        causal,
        SemanticPayload::QuestionAsked(MessageContent {
            message_id: MessageId::from_bytes(indexed_id(0x94, index)),
            sender: local,
            recipient: Some(local),
            body: ContentText::new(body).expect("body validates"),
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: Some(OperationCorrelation::new(
                ProviderId::new("paged-provider").expect("provider validates"),
                ProviderSessionId::new("paged-session").expect("session validates"),
                operation,
            )),
            project_id: None,
        }),
    )
    .sign(&signer(1), auxiliary)
    .expect("local message signs")
}

pub fn authored_project_input(
    index: u16,
    project_id: ProjectId,
    body: &str,
) -> VerifiedSemanticFact {
    let root = FactId::from_bytes(verified_fact().verified_event().event_id());
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([root]).expect("one parent validates"),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
    )
    .expect("local authority validates");
    let local = MailboxAddress::new(
        authority_policy().local_installation(),
        authority_policy().local_human_mailbox(),
    );
    let project_mailbox = MailboxAddress::new(
        authority_policy().local_installation(),
        hq_domain::MailboxId::from_bytes(indexed_id(0x68, index)),
    );
    let mut auxiliary = [0_u8; 32];
    auxiliary[0] = 3;
    auxiliary[30..].copy_from_slice(&index.to_be_bytes());
    CanonicalEventPlan::new(
        authority_policy().local_installation(),
        Timestamp::from_unix_millis(2_000 + i64::from(index)),
        FactScope::InstallationPrivate(authority_policy().local_installation()),
        causal,
        SemanticPayload::AsynchronousMessageSent {
            thread_id: None,
            message: MessageContent {
                message_id: MessageId::from_bytes(indexed_id(0x58, index)),
                sender: local,
                recipient: Some(project_mailbox),
                body: ContentText::new(body).expect("body validates"),
                purpose: MessagePurpose::Asynchronous,
                presentation: PresentationKind::Message,
                correlation: None,
                project_id: Some(project_id),
            },
        },
    )
    .sign(&signer(1), auxiliary)
    .expect("project input signs")
}

fn authored_conversation_entry_with_retention(
    index: u16,
    activity: bool,
    durable: bool,
) -> VerifiedSemanticFact {
    let root = FactId::from_bytes(verified_fact().verified_event().event_id());
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([root]).expect("one parent validates"),
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            root,
        )],
    )
    .expect("local authority validates");
    let local = MailboxAddress::new(
        authority_policy().local_installation(),
        authority_policy().local_human_mailbox(),
    );
    let correlation = OperationCorrelation::new(
        ProviderId::new("paged-provider").expect("provider validates"),
        ProviderSessionId::new("paged-session").expect("session validates"),
        OperationId::from_bytes(indexed_id(0x70, index)),
    );
    let payload = if activity {
        SemanticPayload::HarnessActivityRecorded {
            project: None,
            source: local,
            correlation,
            item: durable
                .then(|| ShortText::new(format!("item-{index:04}")).expect("item validates")),
            kind: if durable {
                ActivityKind::CompletedItem
            } else {
                ActivityKind::Progress
            },
            logical_key: ShortText::new(format!("progress-{index:04}")).expect("key validates"),
            runtime: ShortText::new("runtime-1").expect("runtime validates"),
            sequence: std::num::NonZeroU64::new(u64::from(index) + 1)
                .expect("sequence is positive"),
            occurred_at: Timestamp::from_unix_millis(2_000),
            status: ActivityStatus::Running,
            content: ContentText::new(format!("activity {index}")).expect("content validates"),
            truncated: false,
            completed: durable.then_some(hq_domain::CompletedItemPresentation::Unknown),
        }
    } else {
        SemanticPayload::QuestionAsked(MessageContent {
            message_id: MessageId::from_bytes(indexed_id(0x50, index)),
            sender: local,
            recipient: Some(local),
            body: ContentText::new(format!("message {index}")).expect("body validates"),
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: Some(correlation),
            project_id: None,
        })
    };
    let mut auxiliary = [0_u8; 32];
    auxiliary[0] = u8::from(activity);
    auxiliary[30..].copy_from_slice(&index.to_be_bytes());
    CanonicalEventPlan::new(
        InstallationId::from_bytes([0x11; 32]),
        Timestamp::from_unix_millis(2_000),
        FactScope::InstallationPrivate(InstallationId::from_bytes([0x11; 32])),
        causal,
        payload,
    )
    .sign(&signer(1), auxiliary)
    .expect("typed conversation entry signs")
}

fn indexed_id(prefix: u8, index: u16) -> [u8; 32] {
    let mut value = [prefix; 32];
    value[30..].copy_from_slice(&index.to_be_bytes());
    value
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
