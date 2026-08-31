//! End-to-end single-binary node lifecycle contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::os::unix::fs::symlink;
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    num::NonZeroUsize,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use hq_application::FactPlan;
use hq_domain::{
    AccountId, AuthorityReference, AuthorityRole, BoundedSet, BoundedText, BoundedVec,
    CausalReferences, CommandId, ContentText, FactId, FactScope, InitialProjectState,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxId, MailboxKind, ProjectId, ProjectResource,
    ProviderId, ProviderSessionId, RESOURCE_LOCATOR_MAX_BYTES, RepositoryContext, ResourceHealth,
    ResourceId, ResourceLocator, ResourceScheme, SemanticPayload, ShortText, Timestamp,
};
use hq_local_api::{
    ClientEvent, InitialView,
    protocol::v1::{
        BuildMetadata, LifecycleRequest, LifecycleState, MutationAttemptDto, MutationOutcomeDto,
        MutationRequest, Request, ResponseResult, SnapshotItem,
    },
};
use hq_node::{
    LifecycleClient, LifecycleClientConfig, LocalNodeClient, LocalNodeClientConfig,
    LocalNodeEventClient, ProcessNodeLauncher, RuntimePaths, StateDirectoryOwner, StatePaths,
    execute_cli_with_input,
};

use support::TestDirectory;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct DaemonStopGuard(PathBuf);

impl Drop for DaemonStopGuard {
    fn drop(&mut self) {
        let _ = output("stop", &self.0);
    }
}

struct OutputChildGuard(Option<Child>);

impl OutputChildGuard {
    fn wait_with_output(&mut self, timeout: Duration) -> Output {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .0
                .as_mut()
                .expect("guarded child")
                .try_wait()
                .expect("child status")
                .is_some()
            {
                return self
                    .0
                    .take()
                    .expect("completed child")
                    .wait_with_output()
                    .expect("child output");
            }
            assert!(Instant::now() < deadline, "CLI process timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OutputChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn initialize(directory: &TestDirectory) -> (StatePaths, RuntimePaths) {
    let state = StatePaths::new(directory.path().join("state")).expect("state paths");
    let owner = StateDirectoryOwner::acquire(state.clone()).expect("state owner");
    let _ = owner.initialize().expect("identity");
    drop(owner);
    let runtime = RuntimePaths::new(state.root().join("runtime")).expect("runtime paths");
    (state, runtime)
}

fn command(action: &str, state_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command
        .arg("--state-root")
        .arg(state_root)
        .arg("daemon")
        .arg(action);
    command
}

fn output(action: &str, state_root: &Path) -> Output {
    command(action, state_root)
        .output()
        .expect("CLI process runs")
}

fn machine_output(action: &str, state_root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(state_root)
        .arg("daemon")
        .arg(action)
        .stdin(Stdio::null())
        .output()
        .expect("non-interactive CLI process runs")
}

fn offline_output(
    state_root: &Path,
    arguments: impl IntoIterator<Item = OsString>,
    input: Option<&[u8]>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(state_root)
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn().expect("offline CLI process starts");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input)
            .expect("secret input writes");
    }
    child.wait_with_output().expect("offline CLI process exits")
}

fn initialize_identity(state_root: &Path) -> serde_json::Value {
    let initialized = offline_output(
        state_root,
        [OsString::from("identity"), OsString::from("init")],
        None,
    );
    assert!(
        initialized.status.success(),
        "identity init stderr: {:?}",
        initialized.stderr
    );
    let value: serde_json::Value =
        serde_json::from_slice(&initialized.stdout).expect("identity JSON");
    assert_eq!(value["kind"], "identity");
    assert_eq!(
        value["data"]["installation_id"].as_str().map(str::len),
        Some(64)
    );
    assert_eq!(
        value["data"]["signing_public_key"].as_str().map(str::len),
        Some(64)
    );
    value
}

fn human_output(state_root: &Path, arguments: &[&str]) -> Output {
    offline_output(
        state_root,
        std::iter::once(OsString::from("human"))
            .chain(arguments.iter().copied().map(OsString::from)),
        None,
    )
}

fn message_output(state_root: &Path, arguments: &[&str]) -> Output {
    offline_output(
        state_root,
        arguments.iter().copied().map(OsString::from),
        None,
    )
}

fn admin_output(state_root: &Path, command: &str, arguments: &[&str]) -> Output {
    offline_output(
        state_root,
        std::iter::once(OsString::from(command))
            .chain(arguments.iter().copied().map(OsString::from)),
        None,
    )
}

fn admin_json(state_root: &Path, command: &str, arguments: &[&str]) -> serde_json::Value {
    let output = admin_output(state_root, command, arguments);
    assert!(
        output.status.success(),
        "{command} stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("admin JSON")
}

fn agent_output(state_root: &Path, arguments: &[&str]) -> Output {
    admin_output(state_root, "agent", arguments)
}

fn agent_json(state_root: &Path, arguments: &[&str]) -> serde_json::Value {
    admin_json(state_root, "agent", arguments)
}

fn harness_output(state_root: &Path, arguments: &[&str]) -> Output {
    admin_output(state_root, "harness", arguments)
}

fn project_json(state_root: &Path, arguments: &[&str]) -> serde_json::Value {
    admin_json(state_root, "project", arguments)
}

fn encode_hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .into_iter()
        .flat_map(|byte| {
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 0x0f)]),
            ]
        })
        .collect()
}

fn local_client_config(state: StatePaths, initial_view: InitialView) -> LocalNodeClientConfig {
    LocalNodeClientConfig {
        state,
        build: BuildMetadata::new("hq-test", "0.1.0", Some("cli-e2e")).expect("build"),
        initial_view,
        io_timeout: Duration::from_secs(2),
        command_deadline: Duration::from_secs(5),
        max_connection_attempts: NonZeroUsize::new(8).expect("positive attempts"),
        readiness_timeout: Duration::from_secs(5),
        readiness_retry_interval: Duration::from_millis(10),
        reconnect_initial: Duration::from_millis(10),
        reconnect_maximum: Duration::from_millis(40),
        completed_identity_capacity: NonZeroUsize::new(16).expect("positive history"),
    }
}

fn local_client(state: StatePaths, initial_view: InitialView) -> LocalNodeClient {
    LocalNodeClient::connect_with_launcher(
        local_client_config(state, initial_view),
        ProcessNodeLauncher::new(env!("CARGO_BIN_EXE_hq").into()),
    )
    .expect("local command client")
}

fn local_event_client(state: StatePaths) -> LocalNodeEventClient {
    LocalNodeEventClient::connect_with_launcher(
        local_client_config(state, InitialView::OnDemand),
        ProcessNodeLauncher::new(env!("CARGO_BIN_EXE_hq").into()),
    )
    .expect("subscribed local event client")
}

fn commit_plan(client: &mut LocalNodeClient, identity: u8, plan: FactPlan) {
    let request = MutationRequest::from_plan(CommandId::from_bytes([identity; 32]), plan)
        .expect("mutation request");
    assert!(matches!(
        client.mutation(request).expect("mutation completes"),
        ClientEvent::Mutation(MutationAttemptDto::Completed {
            outcome: MutationOutcomeDto::Committed,
            ..
        })
    ));
}

#[allow(clippy::too_many_lines)]
fn setup_direct_agent_session(
    client: &mut LocalNodeClient,
    directory: &Path,
) -> (MailboxId, ProviderId, ProviderSessionId) {
    let local = client.installation_id();
    let snapshot = client.snapshot().expect("initial snapshot");
    let root_fact = snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Installation {
                installation_id,
                root_fact,
                ..
            } if installation_id.bytes() == *local.as_bytes() => {
                Some(FactId::from_bytes(root_fact.bytes()))
            }
            _ => None,
        })
        .expect("installation root");
    let agent_mailbox = MailboxId::from_bytes([0xa3; 32]);
    let authority = [AuthorityReference::new(
        AuthorityRole::LocalInstallation,
        root_fact,
    )];
    let root_causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([root_fact]).expect("root parent"),
        authority,
    )
    .expect("root authority");
    commit_plan(
        client,
        0xa1,
        FactPlan::new(
            local,
            Timestamp::from_unix_millis(10),
            FactScope::InstallationPrivate(local),
            root_causal,
            SemanticPayload::MailboxCreated {
                mailbox_id: agent_mailbox,
                kind: MailboxKind::Agent,
                label: None,
            },
            [0xa1; 32],
        ),
    );
    let snapshot = client.snapshot().expect("mailbox snapshot");
    let mailbox_fact = snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Mailbox {
                installation_id,
                mailbox_id,
                create_fact,
                ..
            } if installation_id.bytes() == *local.as_bytes()
                && mailbox_id.bytes() == *agent_mailbox.as_bytes() =>
            {
                Some(FactId::from_bytes(create_fact.bytes()))
            }
            _ => None,
        })
        .expect("agent mailbox fact");
    let provider = ProviderId::new("codex").expect("provider");
    let session = ProviderSessionId::new("mailbox-e2e").expect("session");
    let causal = || {
        CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
            BoundedSet::new([root_fact, mailbox_fact]).expect("binding parents"),
            authority,
        )
        .expect("binding authority")
    };
    commit_plan(
        client,
        0xa2,
        FactPlan::new(
            local,
            Timestamp::from_unix_millis(11),
            FactScope::InstallationPrivate(local),
            causal(),
            SemanticPayload::MailboxSessionBound {
                mailbox_id: agent_mailbox,
                provider: provider.clone(),
                session: session.clone(),
            },
            [0xa2; 32],
        ),
    );
    let directory = fs::canonicalize(directory).expect("context directory canonicalizes");
    commit_plan(
        client,
        0xa3,
        FactPlan::new(
            local,
            Timestamp::from_unix_millis(12),
            FactScope::InstallationPrivate(local),
            causal(),
            SemanticPayload::MailboxContextRecorded {
                mailbox_id: agent_mailbox,
                context: RepositoryContext {
                    directory: ResourceLocator::new(
                        ResourceScheme::WorkingTree,
                        BoundedText::<RESOURCE_LOCATOR_MAX_BYTES>::new(
                            directory.to_str().expect("UTF-8 directory").to_owned(),
                        )
                        .expect("directory locator"),
                    ),
                    repository: None,
                    worktree: None,
                    branch: None,
                },
            },
            [0xa3; 32],
        ),
    );
    (agent_mailbox, provider, session)
}

#[test]
fn subscribed_local_event_client_refreshes_and_reconnects_to_a_real_node() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _stop = DaemonStopGuard(state_root.clone());
    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let mut subscriber = local_event_client(state);
    let initial_revision = subscriber.snapshot().expect("initial snapshot").revision;
    let initial_connection = subscriber.connection_state();

    let created = human_output(&state_root, &["create", "Subscribed"]);
    assert!(
        created.status.success(),
        "human create stderr: {:?}",
        created.stderr
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = subscriber
            .poll_event(Duration::from_millis(100))
            .expect("subscribed client poll");
        if matches!(
            event,
            Some(ClientEvent::Snapshot(ref snapshot)) if snapshot.revision > initial_revision
        ) {
            assert_eq!(
                subscriber.connection_state(),
                initial_connection,
                "an ordinary commit must invalidate the live subscription without reconnecting"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "subscribed client did not refresh after invalidation"
        );
    }

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = subscriber
            .poll_event(Duration::from_millis(100))
            .expect("reconnecting subscribed client poll");
        if subscriber.connection_state() != initial_connection
            && matches!(event, Some(ClientEvent::Snapshot(_)))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "subscribed client did not reconnect with a fresh snapshot"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn named_agent_catalog_reconciles_create_adopt_selection_and_rename_across_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);

    let created = agent_json(&state_root, &["create", "build-agent"]);
    assert_eq!(created["kind"], "named_agents");
    assert_eq!(created["data"]["operation"], "agent_create");
    assert_eq!(created["data"]["agents"][0]["names"][0], "build-agent");
    let created_agent = created["data"]["agents"][0]["agent_id"]
        .as_str()
        .expect("created agent identity")
        .to_owned();
    let created_mailbox = created["data"]["agents"][0]["mailboxes"][0]["mailbox_id"]
        .as_str()
        .expect("created mailbox identity")
        .to_owned();

    let replayed = agent_json(&state_root, &["create", "build-agent"]);
    assert_eq!(replayed["data"]["agents"][0]["agent_id"], created_agent);
    assert_eq!(
        replayed["data"]["agents"][0]["mailboxes"][0]["mailbox_id"],
        created_mailbox
    );
    let shown = agent_json(&state_root, &["show", "build-agent"]);
    assert_eq!(shown["data"]["agents"].as_array().map(Vec::len), Some(1));

    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let mut seed = local_client(state, InitialView::Snapshot);
    let (mailbox, provider, session) = setup_direct_agent_session(&mut seed, directory.path());
    drop(seed);
    let mailbox = encode_hex(*mailbox.as_bytes());
    let adopted = agent_json(
        &state_root,
        &["create", "review-agent", "--mailbox", &mailbox],
    );
    assert_eq!(adopted["data"]["agents"][0]["names"][0], "review-agent");
    assert_eq!(
        adopted["data"]["agents"][0]["mailboxes"][0]["mailbox_id"],
        mailbox
    );

    let canonical_directory = fs::canonicalize(directory.path()).expect("canonical test path");
    let selected = agent_output(
        &state_root,
        &[
            "select",
            "review-agent",
            "--provider",
            provider.as_str(),
            "--session",
            session.as_str(),
            "--dir",
            canonical_directory.to_str().expect("UTF-8 path"),
        ],
    );
    assert!(
        selected.status.success(),
        "agent select stderr: {:?}",
        selected.stderr
    );
    let renamed = agent_output(
        &state_root,
        &[
            "rename",
            "review-agent",
            "review work",
            "--provider",
            provider.as_str(),
            "--session",
            session.as_str(),
        ],
    );
    assert!(
        renamed.status.success(),
        "agent rename stderr: {:?}",
        renamed.stderr
    );
    let renamed: serde_json::Value = serde_json::from_slice(&renamed.stdout).expect("rename JSON");
    assert_eq!(renamed["data"]["agents"][0]["runnable"], true);
    assert_eq!(
        renamed["data"]["agents"][0]["sessions"][0]["display_name"],
        "review work"
    );
    assert_eq!(
        renamed["data"]["agents"][0]["sessions"][0]["selected"],
        true
    );

    let mut current = Command::new(env!("CARGO_BIN_EXE_hq"));
    current
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(&state_root)
        .arg("agent")
        .arg("current")
        .env("CODEX_THREAD_ID", session.as_str())
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("PI_SESSION_ID")
        .env_remove("HQ_PROVIDER")
        .env_remove("HQ_SESSION");
    let current = current.output().expect("current command runs");
    assert!(
        current.status.success(),
        "agent current stderr: {:?}",
        current.stderr
    );
    let current: serde_json::Value = serde_json::from_slice(&current.stdout).expect("current JSON");
    assert_eq!(current["data"]["current"]["provider"], "codex");
    assert_eq!(current["data"]["current"]["session"], session.as_str());

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let listed = agent_json(&state_root, &["list"]);
    assert_eq!(listed["data"]["agents"].as_array().map(Vec::len), Some(2));
    assert!(listed["data"]["agents"].as_array().is_some_and(|agents| {
        agents.iter().any(|agent| {
            agent["names"][0] == "review-agent"
                && agent["sessions"][0]["display_name"] == "review work"
        })
    }));
    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
fn idle_named_agent_retirement_is_explicit_and_survives_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let human = human_output(&state_root, &["create"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);

    let created = agent_json(&state_root, &["create", "finished-agent"]);
    let agent_id = created["data"]["agents"][0]["agent_id"]
        .as_str()
        .expect("created agent identity")
        .to_owned();

    let unconfirmed = agent_output(&state_root, &["retire", "finished-agent"]);
    assert!(!unconfirmed.status.success());
    assert!(
        String::from_utf8(unconfirmed.stderr)
            .expect("UTF-8 diagnostic")
            .contains("usage")
    );

    let retired = agent_json(&state_root, &["retire", "finished-agent", "--yes"]);
    assert_eq!(retired["kind"], "named_agent_retirement");
    assert_eq!(retired["data"]["agent_id"], agent_id);
    assert_eq!(retired["data"]["force"], false);
    assert!(retired["data"]["project_id"].is_null());
    assert!(retired["data"]["runtime"].is_null());
    assert!(retired["data"]["runtime_code"].is_null());

    let listed = agent_json(&state_root, &["list"]);
    assert!(listed["data"]["agents"].as_array().is_some_and(|agents| {
        agents
            .iter()
            .any(|agent| agent["agent_id"] == agent_id && agent["lifecycle"] == "retired")
    }));

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let after_restart = agent_json(&state_root, &["list"]);
    assert!(
        after_restart["data"]["agents"]
            .as_array()
            .is_some_and(|agents| agents
                .iter()
                .any(|agent| { agent["agent_id"] == agent_id && agent["lifecycle"] == "retired" }))
    );

    let repeated = agent_output(&state_root, &["retire", &agent_id, "--yes"]);
    assert!(!repeated.status.success());
    assert!(
        String::from_utf8(repeated.stderr)
            .expect("UTF-8 diagnostic")
            .contains("agent.state_unavailable")
    );
    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
#[allow(clippy::too_many_lines, reason = "complete real-node project fixture")]
fn project_catalog_reads_authoritative_state_and_survives_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);

    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let mut client = local_client(state, InitialView::OnDemand);
    let local = client.installation_id();
    let snapshot = client.snapshot().expect("human snapshot");
    let root_fact = snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Installation {
                installation_id,
                root_fact,
                ..
            } if installation_id.bytes() == *local.as_bytes() => {
                Some(FactId::from_bytes(root_fact.bytes()))
            }
            _ => None,
        })
        .expect("installation root");
    let (account_id, active_human) = snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Account {
                account_id,
                root_fact,
                selected: true,
                ..
            } => Some((
                AccountId::from_bytes(account_id.bytes()),
                FactId::from_bytes(root_fact.bytes()),
            )),
            _ => None,
        })
        .expect("selected account");
    let project_id = ProjectId::from_bytes([0xd1; 32]);
    let resource_id = ResourceId::from_bytes([0xd2; 32]);
    let locator = ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new("/workspace/catalog".to_owned()).expect("resource locator"),
    );
    let project = ProjectResource {
        resource_id,
        display_locator: locator.clone(),
        canonical_locator: locator,
        health: ResourceHealth::Healthy,
    };
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        BoundedSet::new([root_fact, active_human]).expect("project parents"),
        [
            AuthorityReference::new(AuthorityRole::ProjectHome, root_fact),
            AuthorityReference::new(AuthorityRole::AccountMembership, active_human),
            AuthorityReference::new(AuthorityRole::ActiveHuman, active_human),
        ],
    )
    .expect("project authority");
    commit_plan(
        &mut client,
        0xd1,
        FactPlan::new(
            local,
            Timestamp::from_unix_millis(100),
            FactScope::AccountAddressed(account_id),
            causal,
            SemanticPayload::ProjectCreated {
                project_id,
                mailbox_id: MailboxId::from_bytes([0xd3; 32]),
                home: local,
                name: ShortText::new("Catalog E2E").expect("project name"),
                brief: Some(ContentText::new("restart-safe catalog").expect("brief")),
                predecessor: None,
                resources: BoundedVec::new([project]).expect("project resources"),
                primary: Some(resource_id),
                initial_state: InitialProjectState::Open,
            },
            [0xd1; 32],
        ),
    );

    let listed = project_json(&state_root, &["list"]);
    assert_eq!(listed["kind"], "project_catalog");
    assert_eq!(listed["data"]["operation"], "list");
    assert_eq!(listed["data"]["projects"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["data"]["projects"][0]["name"], "Catalog E2E");
    assert_eq!(listed["data"]["projects"][0]["lifecycle"], "open");
    assert_eq!(
        listed["data"]["projects"][0]["resources"][0]["health"],
        "healthy"
    );
    assert_eq!(
        listed["data"]["projects"][0]["resources"][0]["primary"],
        true
    );

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let shown = project_json(&state_root, &["show", &encode_hex([0xd1; 32])]);
    assert_eq!(shown["data"]["operation"], "show");
    assert_eq!(
        shown["data"]["projects"][0]["project_id"],
        encode_hex([0xd1; 32])
    );
    assert_eq!(shown["data"]["projects"][0]["name"], "Catalog E2E");

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
fn bare_noninteractive_invocation_renders_the_human_inbox() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _stop = DaemonStopGuard(state_root.clone());
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);

    let listed = offline_output(&state_root, std::iter::empty::<OsString>(), None);
    assert!(listed.status.success(), "bare stderr: {:?}", listed.stderr);
    let listed: serde_json::Value =
        serde_json::from_slice(&listed.stdout).expect("bare inbox JSON");
    assert_eq!(listed["kind"], "messages");
    assert_eq!(listed["data"]["operation"], "list");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "complete real-node creation race and restart"
)]
fn project_create_claims_one_existing_path_and_survives_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let worktree = directory.path().join("existing-worktree");
    fs::create_dir(&worktree).expect("existing working tree");
    initialize_identity(&state_root);
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);

    let unknown_home = encode_hex([0xee; 32]);
    let stale_home = admin_output(
        &state_root,
        "project",
        &[
            "create",
            "Wrong home",
            "--path",
            worktree.to_str().expect("UTF-8 test path"),
            "--home",
            &unknown_home,
        ],
    );
    assert_eq!(stale_home.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&stale_home.stderr).contains("project.state_unavailable"),
        "stale home stderr: {:?}",
        stale_home.stderr
    );

    let first_root = state_root.clone();
    let first_path = worktree.clone();
    let first = std::thread::spawn(move || {
        admin_output(
            &first_root,
            "project",
            &[
                "create",
                "First claimant",
                "--brief",
                "created over an existing path",
                "--path",
                first_path.to_str().expect("UTF-8 test path"),
            ],
        )
    });
    let second_root = state_root.clone();
    let second_path = worktree.clone();
    let second = std::thread::spawn(move || {
        admin_output(
            &second_root,
            "project",
            &[
                "create",
                "Second claimant",
                "--path",
                second_path.to_str().expect("UTF-8 test path"),
            ],
        )
    });
    let first = first.join().expect("first creator");
    let second = second.join().expect("second creator");
    assert_eq!(
        usize::from(first.status.success()) + usize::from(second.status.success()),
        1,
        "exactly one create succeeds; first={first:?} second={second:?}"
    );
    let (created, rejected) = if first.status.success() {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(rejected.status.code(), Some(1));
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).expect("created JSON");
    let rejected: serde_json::Value =
        serde_json::from_slice(&rejected.stdout).expect("rejected JSON");
    assert_eq!(created["kind"], "project_operation");
    assert_eq!(created["data"]["operation"], "create");
    assert_eq!(created["data"]["status"], "completed");
    assert_eq!(
        created["data"]["project_id"].as_str().map(str::len),
        Some(64)
    );
    assert_eq!(
        created["data"]["project_head"].as_str().map(str::len),
        Some(64)
    );
    assert_eq!(rejected["kind"], "project_operation");
    assert_eq!(rejected["data"]["status"], "rejected");
    assert_eq!(
        rejected["data"]["error_code"],
        "project_creation_resource_conflict"
    );

    let listed = project_json(&state_root, &["list"]);
    assert_eq!(listed["data"]["projects"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        listed["data"]["projects"][0]["resources"][0]["display_locator"]["value"],
        worktree.to_str().expect("UTF-8 test path")
    );
    assert_eq!(
        listed["data"]["projects"][0]["resources"][0]["canonical_locator"]["value"],
        fs::canonicalize(&worktree)
            .expect("canonical worktree")
            .to_str()
            .expect("UTF-8 canonical path")
    );
    let project_id = created["data"]["project_id"]
        .as_str()
        .expect("project id")
        .to_owned();

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let shown = project_json(&state_root, &["show", &project_id]);
    assert_eq!(shown["data"]["projects"][0]["project_id"], project_id);
    assert_eq!(shown["data"]["projects"][0]["lifecycle"], "open");

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "complete foreground exact-base worktree lifecycle"
)]
fn project_worktree_provisions_real_git_state_and_never_cleans_it_up() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let repository = directory.path().join("repository");
    let destination = directory.path().join("provisioned-worktree");
    fs::create_dir(&repository).expect("repository creates");
    let git = |working_tree: &Path, arguments: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(working_tree)
            .args(arguments)
            .output()
            .expect("Git runs");
        assert!(output.status.success(), "Git failed: {output:?}");
        String::from_utf8(output.stdout)
            .expect("Git output is UTF-8")
            .trim()
            .to_owned()
    };
    git(&repository, &["init", "-q"]);
    git(&repository, &["config", "user.email", "hq@example.invalid"]);
    git(&repository, &["config", "user.name", "HQ Test"]);
    fs::write(repository.join("tracked.txt"), "initial\n").expect("initial content writes");
    git(&repository, &["add", "tracked.txt"]);
    git(&repository, &["commit", "-qm", "initial"]);
    let base = git(&repository, &["rev-parse", "HEAD"]);
    fs::write(repository.join("tracked.txt"), "later\n").expect("later content writes");
    git(&repository, &["commit", "-qam", "later"]);

    initialize_identity(&state_root);
    let _stop = DaemonStopGuard(state_root.clone());
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);
    let provisioned = project_json(
        &state_root,
        &[
            "worktree",
            "Exact base project",
            "--brief",
            "recoverable real Git provisioning",
            "--source",
            repository.to_str().expect("UTF-8 repository"),
            "--destination",
            destination.to_str().expect("UTF-8 destination"),
            "--branch",
            "feature/exact-base",
            "--create-branch",
            &base,
        ],
    );
    assert_eq!(provisioned["kind"], "project_operation");
    assert_eq!(provisioned["data"]["operation"], "worktree");
    assert_eq!(provisioned["data"]["status"], "completed");
    assert!(provisioned["data"]["external_state_warning"].is_null());
    let project_id = provisioned["data"]["project_id"]
        .as_str()
        .expect("project identity")
        .to_owned();

    assert_eq!(git(&destination, &["rev-parse", "HEAD"]), base);
    assert_eq!(
        git(&destination, &["branch", "--show-current"]),
        "feature/exact-base"
    );
    assert_eq!(
        fs::read_to_string(destination.join("tracked.txt")).expect("worktree content reads"),
        "initial\n"
    );
    let shown = project_json(&state_root, &["show", &project_id]);
    assert_eq!(
        shown["data"]["projects"][0]["resources"][0]["display_locator"]["value"],
        destination.to_str().expect("UTF-8 destination")
    );

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let after_restart = project_json(&state_root, &["show", &project_id]);
    assert_eq!(
        after_restart["data"]["projects"][0]["name"],
        "Exact base project"
    );

    let closed = project_json(&state_root, &["close", &project_id, "--yes"]);
    assert_eq!(closed["data"]["status"], "completed");
    let archived = project_json(&state_root, &["archive", &project_id]);
    assert_eq!(archived["data"]["status"], "completed");
    assert!(destination.join(".git").is_file());
    assert_eq!(
        git(
            &repository,
            &["show-ref", "--hash", "refs/heads/feature/exact-base"]
        ),
        base
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "complete real Git, symlink, and restart resource check"
)]
fn project_resource_checks_observe_git_and_symlink_changes_across_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let first_target = directory.path().join("first-target");
    let second_target = directory.path().join("second-target");
    let selected_path = directory.path().join("selected-worktree");
    fs::create_dir(&first_target).expect("first target");
    fs::create_dir(&second_target).expect("second target");
    symlink(&first_target, &selected_path).expect("selected symlink");
    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&first_target)
            .args(arguments)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git stderr: {:?}", output.stderr);
    };
    git(&["init"]);
    git(&["config", "user.name", "HQ Test"]);
    git(&["config", "user.email", "hq@example.invalid"]);
    fs::write(first_target.join("tracked.txt"), "clean\n").expect("tracked file");
    git(&["add", "tracked.txt"]);
    git(&["commit", "-m", "initial"]);

    initialize_identity(&state_root);
    let _stop = DaemonStopGuard(state_root.clone());
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);
    let created = project_json(
        &state_root,
        &[
            "create",
            "Observed resource",
            "--path",
            selected_path.to_str().expect("UTF-8 selected path"),
        ],
    );
    let project_id = created["data"]["project_id"]
        .as_str()
        .expect("project id")
        .to_owned();

    let listed = project_json(&state_root, &["resource", "list", &project_id]);
    assert_eq!(listed["kind"], "project_resources");
    assert_eq!(listed["data"]["operation"], "resource_list");
    assert_eq!(
        listed["data"]["resources"][0]["display_locator"]["value"],
        selected_path.to_str().expect("UTF-8 selected path")
    );
    assert_eq!(
        listed["data"]["resources"][0]["canonical_locator"]["value"],
        fs::canonicalize(&first_target)
            .expect("canonical first target")
            .to_str()
            .expect("UTF-8 first target")
    );
    let resource_id = listed["data"]["resources"][0]["resource_id"]
        .as_str()
        .expect("resource id")
        .to_owned();
    let shown = project_json(
        &state_root,
        &["resource", "show", &project_id, &resource_id],
    );
    assert_eq!(shown["data"]["resources"][0]["resource_id"], resource_id);

    let clean = project_json(&state_root, &["check", &project_id, &resource_id]);
    assert_eq!(clean["kind"], "project_check");
    assert_eq!(clean["data"]["checks"][0]["status"], "accepted");
    assert_eq!(clean["data"]["checks"][0]["health"], "healthy");
    assert_eq!(clean["data"]["checks"][0]["release"], "clean");
    assert_eq!(clean["data"]["checks"][0]["resource_id"], resource_id);

    fs::write(first_target.join("tracked.txt"), "dirty\n").expect("dirty tracked file");
    let dirty = project_json(&state_root, &["check", &project_id]);
    assert_eq!(dirty["data"]["checks"][0]["health"], "healthy");
    assert_eq!(dirty["data"]["checks"][0]["release"], "dirty");

    fs::remove_file(&selected_path).expect("remove old symlink");
    symlink(&second_target, &selected_path).expect("retarget selected symlink");
    let moved = project_json(&state_root, &["check", &project_id]);
    assert_eq!(moved["data"]["checks"][0]["health"], "degraded");
    assert_eq!(
        moved["data"]["checks"][0]["observed_canonical"]["value"],
        fs::canonicalize(&second_target)
            .expect("canonical second target")
            .to_str()
            .expect("UTF-8 second target")
    );
    assert_eq!(moved["data"]["checks"][0]["release"], "unknown");

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let after_restart = project_json(&state_root, &["check", &project_id, &resource_id]);
    assert_eq!(
        after_restart["data"]["checks"][0]["resource_id"],
        resource_id
    );
    assert_eq!(after_restart["data"]["checks"][0]["health"], "degraded");

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "complete foreground desired-resource mutation lifecycle"
)]
fn project_resource_mutations_preserve_external_paths_and_survive_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let original = directory.path().join("original");
    let added = directory.path().join("added");
    let replacement = directory.path().join("replacement");
    let overlapping = added.join("nested");
    for path in [&original, &added, &replacement, &overlapping] {
        fs::create_dir_all(path).expect("resource directory");
        fs::write(path.join("owned-by-human.txt"), "untouched\n").expect("resource marker");
    }

    initialize_identity(&state_root);
    let _stop = DaemonStopGuard(state_root.clone());
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);
    let created = project_json(
        &state_root,
        &[
            "create",
            "Mutable resources",
            "--path",
            original.to_str().expect("UTF-8 original"),
        ],
    );
    let project_id = created["data"]["project_id"]
        .as_str()
        .expect("project id")
        .to_owned();
    let original_id = project_json(&state_root, &["resource", "list", &project_id])["data"]
        ["resources"][0]["resource_id"]
        .as_str()
        .expect("original resource")
        .to_owned();

    let add = project_json(
        &state_root,
        &[
            "resource",
            "add",
            &project_id,
            "--path",
            added.to_str().expect("UTF-8 added"),
            "--primary",
        ],
    );
    assert_eq!(add["data"]["status"], "completed");
    assert_eq!(add["data"]["operation"], "resource_add");
    let resources = project_json(&state_root, &["resource", "list", &project_id]);
    assert_eq!(
        resources["data"]["resources"].as_array().map(Vec::len),
        Some(2)
    );
    let added_resource = resources["data"]["resources"]
        .as_array()
        .expect("resources")
        .iter()
        .find(|resource| {
            resource["display_locator"]["value"] == added.to_str().expect("UTF-8 added path")
        })
        .expect("added resource");
    assert_eq!(added_resource["primary"], true);
    let added_id = added_resource["resource_id"]
        .as_str()
        .expect("added resource id")
        .to_owned();

    let conflict = admin_output(
        &state_root,
        "project",
        &[
            "create",
            "Overlap",
            "--path",
            overlapping.to_str().expect("UTF-8 overlap"),
        ],
    );
    assert_eq!(conflict.status.code(), Some(1));
    let conflict: serde_json::Value =
        serde_json::from_slice(&conflict.stdout).expect("conflict JSON");
    assert_eq!(conflict["data"]["status"], "rejected");
    assert_eq!(
        conflict["data"]["error_code"],
        "project_creation_resource_conflict"
    );

    let primary = project_json(
        &state_root,
        &["resource", "primary", &project_id, &original_id],
    );
    assert_eq!(primary["data"]["status"], "completed");
    let replace = project_json(
        &state_root,
        &[
            "resource",
            "replace",
            &project_id,
            &added_id,
            "--path",
            replacement.to_str().expect("UTF-8 replacement"),
        ],
    );
    assert_eq!(replace["data"]["status"], "completed");
    let replaced = project_json(&state_root, &["resource", "list", &project_id]);
    let replacement_id = replaced["data"]["resources"]
        .as_array()
        .expect("replaced resources")
        .iter()
        .find(|resource| {
            resource["display_locator"]["value"]
                == replacement.to_str().expect("UTF-8 replacement path")
        })
        .and_then(|resource| resource["resource_id"].as_str())
        .expect("replacement resource id")
        .to_owned();
    assert_ne!(replacement_id, added_id);

    let removed = project_json(
        &state_root,
        &["resource", "remove", &project_id, &replacement_id],
    );
    assert_eq!(removed["data"]["status"], "completed");
    for path in [&original, &added, &replacement, &overlapping] {
        assert_eq!(
            fs::read_to_string(path.join("owned-by-human.txt")).expect("marker remains"),
            "untouched\n"
        );
    }

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let after_restart = project_json(&state_root, &["resource", "list", &project_id]);
    assert_eq!(
        after_restart["data"]["resources"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        after_restart["data"]["resources"][0]["resource_id"],
        original_id
    );

    let closed = project_json(&state_root, &["close", &project_id, "--yes"]);
    assert_eq!(closed["data"]["status"], "completed");
    let archived = project_json(&state_root, &["archive", &project_id]);
    assert_eq!(archived["data"]["status"], "completed");
    for path in [&original, &added, &replacement, &overlapping] {
        assert!(path.join("owned-by-human.txt").is_file());
    }

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "complete foreground project lifecycle"
)]
fn project_lifecycle_runs_in_foreground_and_survives_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let worktree = directory.path().join("lifecycle-worktree");
    fs::create_dir(&worktree).expect("existing working tree");
    initialize_identity(&state_root);
    let foreground = command("run", &state_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("foreground project node starts");
    let mut foreground = ChildGuard(foreground);
    let runtime = RuntimePaths::new(state_root.join("runtime")).expect("runtime paths");
    wait_ready(&mut client(runtime));
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);
    let created = project_json(
        &state_root,
        &[
            "create",
            "Lifecycle target",
            "--path",
            worktree.to_str().expect("UTF-8 worktree"),
        ],
    );
    let project_id = created["data"]["project_id"]
        .as_str()
        .expect("project identity")
        .to_owned();
    let initial = project_json(&state_root, &["show", &project_id]);
    let resources = initial["data"]["projects"][0]["resources"].clone();

    for arguments in [
        vec!["close", project_id.as_str()],
        vec!["close", project_id.as_str(), "--force"],
    ] {
        let unconfirmed = admin_output(&state_root, "project", &arguments);
        assert_eq!(unconfirmed.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&unconfirmed.stderr).contains("cli.arguments"));
    }

    let closed = project_json(&state_root, &["close", &project_id, "--yes"]);
    assert_eq!(closed["data"]["operation"], "close");
    assert_eq!(closed["data"]["status"], "completed");
    let closed_catalog = project_json(&state_root, &["show", &project_id]);
    assert_eq!(closed_catalog["data"]["projects"][0]["lifecycle"], "closed");
    assert_eq!(
        closed_catalog["data"]["projects"][0]["resources"][0]["resource_id"],
        resources[0]["resource_id"]
    );
    assert_eq!(
        closed_catalog["data"]["projects"][0]["resources"][0]["active_claim"],
        false
    );

    let archived = project_json(&state_root, &["archive", &project_id]);
    assert_eq!(archived["data"]["operation"], "archive");
    assert_eq!(archived["data"]["status"], "completed");
    let archived_catalog = project_json(&state_root, &["show", &project_id]);
    assert_eq!(
        archived_catalog["data"]["projects"][0]["lifecycle"],
        "closed"
    );
    assert_eq!(archived_catalog["data"]["projects"][0]["archived"], true);
    assert_eq!(
        archived_catalog["data"]["projects"][0]["resources"],
        closed_catalog["data"]["projects"][0]["resources"]
    );

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );

    let unarchived = project_json(&state_root, &["unarchive", &project_id]);
    assert_eq!(unarchived["data"]["operation"], "unarchive");
    assert_eq!(unarchived["data"]["status"], "completed");
    let unarchived_catalog = project_json(&state_root, &["show", &project_id]);
    assert_eq!(
        unarchived_catalog["data"]["projects"][0]["lifecycle"],
        "closed"
    );
    assert_eq!(unarchived_catalog["data"]["projects"][0]["archived"], false);

    let opened = project_json(&state_root, &["open", &project_id]);
    assert_eq!(opened["data"]["operation"], "open");
    assert_eq!(opened["data"]["status"], "completed");
    let opened_catalog = project_json(&state_root, &["show", &project_id]);
    assert_eq!(opened_catalog["data"]["projects"][0]["lifecycle"], "open");
    assert_eq!(
        opened_catalog["data"]["projects"][0]["resources"][0]["resource_id"],
        resources[0]["resource_id"]
    );
    assert_eq!(
        opened_catalog["data"]["projects"][0]["resources"][0]["active_claim"],
        true
    );

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "final stop stderr: {:?}",
        stopped.stderr
    );
    wait_child(&mut foreground.0);
}

#[test]
fn project_send_sequences_argument_and_stdin_work_and_survives_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let worktree = directory.path().join("message-worktree");
    fs::create_dir(&worktree).expect("existing working tree");
    initialize_identity(&state_root);
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);
    let created = project_json(
        &state_root,
        &[
            "create",
            "Message target",
            "--path",
            worktree.to_str().expect("UTF-8 worktree"),
        ],
    );
    let project_id = created["data"]["project_id"]
        .as_str()
        .expect("project identity")
        .to_owned();

    let argument = admin_output(
        &state_root,
        "project",
        &["send", &project_id, "first queued instruction"],
    );
    assert!(
        argument.status.success(),
        "argument send stderr: {:?}",
        argument.stderr
    );
    let argument: serde_json::Value =
        serde_json::from_slice(&argument.stdout).expect("argument send JSON");
    assert_eq!(argument["kind"], "messages");
    assert_eq!(argument["data"]["operation"], "project_send");
    assert_eq!(argument["data"]["project_id"], project_id);
    assert_eq!(
        argument["data"]["root_message"].as_str().map(str::len),
        Some(64)
    );

    let stdin = offline_output(
        &state_root,
        [
            OsString::from("project"),
            OsString::from("send"),
            OsString::from(&project_id),
        ],
        Some(b"second queued instruction\n"),
    );
    assert!(
        stdin.status.success(),
        "stdin send stderr: {:?}",
        stdin.stderr
    );
    let stdin: serde_json::Value = serde_json::from_slice(&stdin.stdout).expect("stdin send JSON");
    assert_eq!(stdin["data"]["project_id"], project_id);

    let shown = project_json(&state_root, &["show", &project_id]);
    assert_eq!(shown["data"]["projects"][0]["input_sequence"], 2);
    let inputs = shown["data"]["projects"][0]["inputs"]
        .as_array()
        .expect("accepted inputs");
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0]["sequence"], 1);
    assert_eq!(inputs[1]["sequence"], 2);
    assert_eq!(inputs[0]["message_id"], argument["data"]["root_message"]);
    assert_eq!(inputs[1]["message_id"], stdin["data"]["root_message"]);

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let restarted = project_json(&state_root, &["show", &project_id]);
    assert_eq!(restarted["data"]["projects"][0]["input_sequence"], 2);
    assert_eq!(
        restarted["data"]["projects"][0]["inputs"],
        shown["data"]["projects"][0]["inputs"]
    );

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
fn project_assignment_commands_execute_against_the_foreground_node_and_survive_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let worktree = directory.path().join("activation-worktree");
    fs::create_dir(&worktree).expect("existing working tree");
    initialize_identity(&state_root);
    let _stop = DaemonStopGuard(state_root.clone());
    let human = human_output(&state_root, &["create", "Personal"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);
    let agent = agent_output(&state_root, &["create", "runtime-agent"]);
    assert!(agent.status.success(), "agent stderr: {:?}", agent.stderr);
    let created = project_json(
        &state_root,
        &[
            "create",
            "Activation target",
            "--path",
            worktree.to_str().expect("UTF-8 worktree"),
        ],
    );
    let project_id = created["data"]["project_id"]
        .as_str()
        .expect("project identity")
        .to_owned();
    let sent = admin_output(
        &state_root,
        "project",
        &["send", &project_id, "pending instruction"],
    );
    assert!(sent.status.success(), "send stderr: {:?}", sent.stderr);

    let activation = admin_output(
        &state_root,
        "project",
        &[
            "activate",
            &project_id,
            "--agent",
            "runtime-agent",
            "--provider",
            "unregistered",
            "--new-session",
        ],
    );
    assert_eq!(activation.status.code(), Some(1));
    assert!(
        activation.stderr.is_empty(),
        "activation stderr: {:?}",
        activation.stderr
    );
    let activation: serde_json::Value =
        serde_json::from_slice(&activation.stdout).expect("activation JSON");
    assert_eq!(activation["kind"], "project_operation");
    assert_eq!(activation["data"]["operation"], "activate");
    assert_eq!(activation["data"]["status"], "rejected");
    assert_eq!(
        activation["data"]["error_code"],
        "project_runtime_start_rejected"
    );

    let dispatch = admin_output(&state_root, "project", &["dispatch", &project_id]);
    assert_eq!(dispatch.status.code(), Some(1));
    assert!(dispatch.stderr.is_empty());
    let dispatch: serde_json::Value =
        serde_json::from_slice(&dispatch.stdout).expect("dispatch JSON");
    assert_eq!(dispatch["data"]["operation"], "dispatch");
    assert_eq!(dispatch["data"]["status"], "rejected");

    let handoff = admin_output(
        &state_root,
        "project",
        &[
            "handoff",
            &project_id,
            "--agent",
            "runtime-agent",
            "--provider",
            "unregistered",
            "--new-session",
            "--thread",
            &"44".repeat(32),
            "--yes",
            "--force",
        ],
    );
    assert_eq!(handoff.status.code(), Some(1));
    let handoff: serde_json::Value =
        serde_json::from_slice(&handoff.stderr).expect("handoff error JSON");
    assert_eq!(handoff["kind"], "error");
    assert_eq!(handoff["data"]["code"], "project.state_unavailable");

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let shown = project_json(&state_root, &["show", &project_id]);
    assert!(shown["data"]["projects"][0]["assignment"].is_null());
    assert_eq!(shown["data"]["projects"][0]["input_sequence"], 1);
    assert!(
        shown["data"]["projects"][0]["dispatches"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
}

#[test]
fn managed_harness_stop_and_stale_exact_resume_are_machine_readable_across_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let created = agent_json(&state_root, &["create", "runtime-agent"]);
    let agent_id = created["data"]["agents"][0]["agent_id"]
        .as_str()
        .expect("agent identity")
        .to_owned();

    let stopped = harness_output(
        &state_root,
        &["stop", "--agent", "runtime-agent", "--provider", "codex"],
    );
    assert!(
        stopped.status.success(),
        "harness stop stderr: {:?}",
        stopped.stderr
    );
    let stopped: serde_json::Value = serde_json::from_slice(&stopped.stdout).expect("stop JSON");
    assert_eq!(stopped["kind"], "harness_session");
    assert_eq!(stopped["data"]["agent_id"], agent_id);
    assert_eq!(stopped["data"]["operation"], "stop");
    assert_eq!(stopped["data"]["provider"], "codex");
    assert_eq!(stopped["data"]["status"], "stopped");
    assert_eq!(
        stopped["data"]["operation_id"].as_str().map(str::len),
        Some(64)
    );

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let stale = harness_output(
        &state_root,
        &[
            "resume",
            "--agent",
            "runtime-agent",
            "--provider",
            "codex",
            "--session",
            "missing-session",
            "--dir",
            directory.path().to_str().expect("UTF-8 test directory"),
        ],
    );
    assert_eq!(stale.status.code(), Some(1));
    assert!(stale.stderr.is_empty());
    let stale: serde_json::Value =
        serde_json::from_slice(&stale.stdout).expect("stale resume JSON");
    assert_eq!(stale["kind"], "harness_session");
    assert_eq!(stale["data"]["operation"], "resume");
    assert_eq!(stale["data"]["requested_session"], "missing-session");
    assert_eq!(stale["data"]["status"], "rejected");
    assert_eq!(stale["data"]["error_code"], "managed_session_precondition");

    let stopped_again = harness_output(
        &state_root,
        &["stop", "--agent", &agent_id, "--provider", "codex"],
    );
    assert!(
        stopped_again.status.success(),
        "second harness stop stderr: {:?}",
        stopped_again.stderr
    );
    let stopped_again: serde_json::Value =
        serde_json::from_slice(&stopped_again.stdout).expect("second stop JSON");
    assert_eq!(stopped_again["data"]["status"], "stopped");

    let daemon_stopped = output("stop", &state_root);
    assert!(
        daemon_stopped.status.success(),
        "daemon stop stderr: {:?}",
        daemon_stopped.stderr
    );
}

#[test]
fn named_agent_current_rejects_ambiguous_provider_environment_without_echoing_sessions() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let command_output = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(&state_root)
        .arg("agent")
        .arg("current")
        .env("CODEX_THREAD_ID", "secret-codex-session")
        .env("PI_SESSION_ID", "secret-pi-session")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("HQ_PROVIDER")
        .env_remove("HQ_SESSION")
        .output()
        .expect("ambiguous current command runs");
    assert!(!command_output.status.success());
    let stderr = String::from_utf8(command_output.stderr).expect("UTF-8 diagnostic");
    assert!(stderr.contains("state_unavailable"));
    assert!(!stderr.contains("secret-codex-session"));
    assert!(!stderr.contains("secret-pi-session"));
    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn mailbox_commands_survive_restart_and_preserve_delivery_state() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let human = human_output(&state_root, &["create"]);
    assert!(human.status.success(), "human stderr: {:?}", human.stderr);

    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let mut seed = local_client(state, InitialView::Snapshot);
    let (agent_mailbox, provider, session) =
        setup_direct_agent_session(&mut seed, directory.path());
    drop(seed);

    let ask = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(&state_root)
        .arg("ask")
        .arg("--provider")
        .arg(provider.as_str())
        .arg("--session")
        .arg(session.as_str())
        .arg("--interval")
        .arg("10ms")
        .arg("question across restart")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ask process starts");
    let mut ask = OutputChildGuard(Some(ask));

    let question = {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if ask
                .0
                .as_mut()
                .expect("guarded ask")
                .try_wait()
                .expect("ask status")
                .is_some()
            {
                let output = ask.wait_with_output(Duration::ZERO);
                panic!(
                    "ask exited before its question became visible: status={:?} stdout={:?} stderr={:?}",
                    output.status, output.stdout, output.stderr
                );
            }
            let listed = message_output(&state_root, &["list"]);
            if listed.status.success() {
                let record: serde_json::Value =
                    serde_json::from_slice(&listed.stdout).expect("message list JSON");
                if let Some(message) = record["data"]["messages"].as_array().and_then(|messages| {
                    messages
                        .iter()
                        .find(|message| message["content"] == "question across restart")
                }) {
                    break message["message_id"]
                        .as_str()
                        .expect("question identity")
                        .to_owned();
                }
            }
            assert!(
                Instant::now() < deadline,
                "question did not become visible; status={:?} stdout={:?} stderr={:?}",
                listed.status,
                listed.stdout,
                listed.stderr
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    let get = |message_id: &str| {
        offline_output(
            &state_root,
            [OsString::from("get"), OsString::from(message_id)],
            None,
        )
    };
    let first_get = get(&question);
    let second_get = get(&question);
    assert!(
        first_get.status.success(),
        "get stderr: {:?}",
        first_get.stderr
    );
    assert!(
        second_get.status.success(),
        "get stderr: {:?}",
        second_get.stderr
    );
    let first_get: serde_json::Value =
        serde_json::from_slice(&first_get.stdout).expect("first get JSON");
    let second_get: serde_json::Value =
        serde_json::from_slice(&second_get.stdout).expect("second get JSON");
    assert_eq!(first_get["data"], second_get["data"]);
    assert_eq!(first_get["data"]["messages"][0]["open"], true);

    let discovered = offline_output(
        &state_root,
        [
            OsString::from("mailboxes"),
            OsString::from("--dir"),
            directory.path().as_os_str().to_owned(),
        ],
        None,
    );
    assert!(
        discovered.status.success(),
        "mailboxes stderr: {:?}",
        discovered.stderr
    );
    let discovered: serde_json::Value =
        serde_json::from_slice(&discovered.stdout).expect("mailbox discovery JSON");
    assert_eq!(discovered["data"]["candidates"][0]["provider"], "codex");
    assert_eq!(
        discovered["data"]["candidates"][0]["session"],
        "mailbox-e2e"
    );
    assert_eq!(discovered["data"]["candidates"][0]["directory_match"], true);

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let answered = message_output(&state_root, &["answer", &question, "answer after restart"]);
    assert!(
        answered.status.success(),
        "answer stderr: {:?}",
        answered.stderr
    );

    let ask_output = ask.wait_with_output(Duration::from_secs(15));
    assert!(
        ask_output.status.success(),
        "ask stderr: {:?}",
        ask_output.stderr
    );
    let ask_record: serde_json::Value =
        serde_json::from_slice(&ask_output.stdout).expect("ask JSON");
    assert_eq!(ask_record["data"]["operation"], "ask");
    assert_eq!(ask_record["data"]["root_message"], question);
    assert_eq!(
        ask_record["data"]["messages"][0]["content"],
        "answer after restart"
    );
    assert_eq!(ask_record["data"]["messages"][0]["root_message"], question);
    let answer_message = ask_record["data"]["messages"][0]["message_id"]
        .as_str()
        .expect("answer identity")
        .to_owned();

    let polled = offline_output(
        &state_root,
        [
            OsString::from("poll"),
            OsString::from("--provider"),
            OsString::from(provider.as_str()),
            OsString::from("--session"),
            OsString::from(session.as_str()),
        ],
        None,
    );
    assert_eq!(polled.status.code(), Some(3));
    assert!(polled.stdout.is_empty());
    assert!(polled.stderr.is_empty());

    let restored_answer = message_output(&state_root, &["restore", &answer_message]);
    assert!(
        restored_answer.status.success(),
        "answer restore stderr: {:?}",
        restored_answer.stderr
    );
    let wait_arguments = vec![
        OsString::from("--output"),
        OsString::from("json"),
        OsString::from("--state-root"),
        state_root.as_os_str().to_owned(),
        OsString::from("wait"),
        OsString::from("--provider"),
        OsString::from(provider.as_str()),
        OsString::from("--session"),
        OsString::from(session.as_str()),
        OsString::from(&question),
    ];
    let first_delivery = execute_cli_with_input(wait_arguments.clone(), &mut std::io::empty());
    let second_delivery = execute_cli_with_input(wait_arguments, &mut std::io::empty());
    assert_eq!(first_delivery.exit_code, 0);
    assert_eq!(first_delivery.stdout, second_delivery.stdout);
    assert_eq!(first_delivery.completion, second_delivery.completion);
    assert!(first_delivery.stdout.contains(&answer_message));
    let rearchived_answer = message_output(&state_root, &["archive", &answer_message]);
    assert!(
        rearchived_answer.status.success(),
        "answer archive stderr: {:?}",
        rearchived_answer.stderr
    );

    let sent = offline_output(
        &state_root,
        [
            OsString::from("send"),
            OsString::from("--provider"),
            OsString::from(provider.as_str()),
            OsString::from("--session"),
            OsString::from(session.as_str()),
            OsString::from("asynchronous delivery"),
        ],
        None,
    );
    assert!(sent.status.success(), "send stderr: {:?}", sent.stderr);
    let sent: serde_json::Value = serde_json::from_slice(&sent.stdout).expect("send JSON");
    let asynchronous = sent["data"]["root_message"]
        .as_str()
        .expect("asynchronous identity");

    let sender = encode_hex(*agent_mailbox.as_bytes());
    let filtered = message_output(&state_root, &["list", "--sender", &sender, "--limit", "2"]);
    assert!(
        filtered.status.success(),
        "filtered list stderr: {:?}",
        filtered.stderr
    );
    let filtered: serde_json::Value =
        serde_json::from_slice(&filtered.stdout).expect("filtered list JSON");
    let filtered_messages = filtered["data"]["messages"]
        .as_array()
        .expect("filtered messages");
    assert!(!filtered_messages.is_empty());
    assert!(filtered_messages.len() <= 2);
    assert!(
        filtered_messages
            .iter()
            .all(|message| { message["sender"]["mailbox_id"].as_str() == Some(sender.as_str()) })
    );

    let archived = message_output(&state_root, &["archive", asynchronous]);
    assert!(
        archived.status.success(),
        "archive stderr: {:?}",
        archived.stderr
    );
    let archived_list = message_output(&state_root, &["list", "--archived"]);
    assert!(archived_list.status.success());
    let archived_list: serde_json::Value =
        serde_json::from_slice(&archived_list.stdout).expect("archived list JSON");
    assert!(
        archived_list["data"]["messages"]
            .as_array()
            .expect("archived messages")
            .iter()
            .any(|message| message["message_id"] == asynchronous)
    );

    let restored = message_output(&state_root, &["restore", asynchronous]);
    assert!(
        restored.status.success(),
        "restore stderr: {:?}",
        restored.stderr
    );
    let open_list = message_output(&state_root, &["list"]);
    assert!(open_list.status.success());
    let open_list: serde_json::Value =
        serde_json::from_slice(&open_list.stdout).expect("open list JSON");
    assert!(
        open_list["data"]["messages"]
            .as_array()
            .expect("open messages")
            .iter()
            .any(|message| message["message_id"] == asynchronous)
    );

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

fn client(runtime: RuntimePaths) -> LifecycleClient {
    LifecycleClient::new(LifecycleClientConfig {
        runtime,
        build: BuildMetadata::new("hq-test", "0.1.0", Some("cli-e2e")).expect("build"),
        io_timeout: Duration::from_millis(500),
    })
    .expect("client")
}

fn wait_ready(client: &mut LifecycleClient) -> hq_node::LifecycleObservation {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(observation) = client.request(LifecycleRequest::Readiness)
            && observation.status.state == LifecycleState::Ready
        {
            return observation;
        }
        assert!(Instant::now() < deadline, "node readiness timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_child(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().expect("child status") {
            assert!(status.success(), "foreground node exit: {status}");
            return;
        }
        assert!(Instant::now() < deadline, "foreground node exit timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn foreground_status_restart_and_stop_converge_across_a_fresh_generation() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialize(&directory);
    let child = command("run", state.root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("foreground starts");
    let mut child = ChildGuard(child);
    let mut probe = client(runtime.clone());
    let first = wait_ready(&mut probe);
    assert_eq!(first.status.revision, Some(1));
    let first_nonce = first.readiness.expect("first readiness").boot_nonce;

    let status = output("status", state.root());
    assert!(
        status.status.success(),
        "status stderr: {:?}",
        status.stderr
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("status=ready"));
    let machine = machine_output("status", state.root());
    assert!(
        machine.status.success(),
        "machine stderr: {:?}",
        machine.stderr
    );
    assert!(machine.stderr.is_empty());
    let record: serde_json::Value =
        serde_json::from_slice(&machine.stdout).expect("machine lifecycle record");
    assert_eq!(record["schema"], "hq-cli-output-v1");
    assert_eq!(record["ok"], true);
    assert_eq!(record["kind"], "lifecycle");
    assert_eq!(record["data"]["command"], "status");
    assert_eq!(record["data"]["state"], "ready");

    let mut local = local_client(state.clone(), InitialView::Snapshot);
    let local_status = local
        .request(Request::Lifecycle(LifecycleRequest::Status))
        .expect("local API status");
    assert!(
        matches!(
            local_status,
            ClientEvent::Response {
            result: ResponseResult::Lifecycle(ref status),
                ..
            } if status.state == LifecycleState::Ready
        ),
        "unexpected local status: {local_status:?}"
    );

    let mut old_connection = UnixStream::connect(runtime.socket_file()).expect("old connection");
    old_connection
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let restarted = output("restart", state.root());
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    assert!(String::from_utf8_lossy(&restarted.stdout).contains("restart=ready"));
    let second = wait_ready(&mut probe);
    assert_eq!(second.status.revision, Some(1));
    assert_ne!(
        second.readiness.expect("second readiness").boot_nonce,
        first_nonce
    );
    let mut terminal = [0_u8; 1];
    assert_eq!(
        old_connection
            .read(&mut terminal)
            .expect("old connection closes"),
        0
    );

    let stopped = output("stop", state.root());
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("stopped intent=stopped"));
    wait_child(&mut child.0);
    assert!(!runtime.socket_file().exists());
    assert!(!runtime.readiness_file().exists());
}

#[test]
fn identity_backup_restore_is_noninteractive_redacted_and_does_not_copy_configuration() {
    let directory = TestDirectory::new();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let backup = directory.path().join("identity-backup.json");
    let unused_backup = directory.path().join("unused-backup.json");
    let password = b"correct horse battery staple\n";

    let initialized = initialize_identity(&source);

    let configured = offline_output(
        &source,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("default-provider"),
            OsString::from("codex"),
        ],
        None,
    );
    assert!(configured.status.success());

    let exported = offline_output(
        &source,
        [
            OsString::from("identity"),
            OsString::from("export"),
            backup.clone().into_os_string(),
            OsString::from("--password-stdin"),
        ],
        Some(password),
    );
    assert!(
        exported.status.success(),
        "export stderr: {:?}",
        exported.stderr
    );
    assert!(!String::from_utf8_lossy(&exported.stdout).contains("correct horse"));
    assert!(!String::from_utf8_lossy(&exported.stderr).contains("correct horse"));

    let closed = offline_output(
        &source,
        [
            OsString::from("identity"),
            OsString::from("export"),
            unused_backup.into_os_string(),
            OsString::from("--password-stdin"),
        ],
        None,
    );
    assert_eq!(closed.status.code(), Some(2));
    let closed_error: serde_json::Value =
        serde_json::from_slice(&closed.stderr).expect("closed-stdin error JSON");
    assert_eq!(closed_error["data"]["code"], "identity.secret_input");

    let wrong = offline_output(
        &target,
        [
            OsString::from("identity"),
            OsString::from("import"),
            backup.clone().into_os_string(),
            OsString::from("--password-stdin"),
        ],
        Some(b"wrong password\n"),
    );
    assert!(!wrong.status.success());
    assert!(!String::from_utf8_lossy(&wrong.stderr).contains("wrong password"));

    let imported = offline_output(
        &target,
        [
            OsString::from("identity"),
            OsString::from("import"),
            backup.into_os_string(),
            OsString::from("--password-stdin"),
        ],
        Some(password),
    );
    assert!(
        imported.status.success(),
        "import stderr: {:?}",
        imported.stderr
    );
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.stdout).expect("import identity JSON");
    assert_eq!(imported["data"], initialized["data"]);

    let overwrite = offline_output(
        &target,
        [OsString::from("identity"), OsString::from("init")],
        None,
    );
    assert!(!overwrite.status.success());
    let target_config = offline_output(
        &target,
        [OsString::from("config"), OsString::from("get")],
        None,
    );
    let target_config: serde_json::Value =
        serde_json::from_slice(&target_config.stdout).expect("default configuration JSON");
    assert_eq!(
        target_config["data"]["default_provider"],
        serde_json::Value::Null
    );
    assert_eq!(target_config["data"]["relays"], serde_json::json!([]));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end contract covers preservation across every configuration mutation"
)]
fn typed_configuration_is_canonical_revalidated_and_refuses_a_live_owner() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let _ = initialize_identity(&state_root);
    let provider = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("default-provider"),
            OsString::from("codex"),
        ],
        None,
    );
    assert!(
        provider.status.success(),
        "provider stderr: {:?}",
        provider.stderr
    );
    let theme = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("theme"),
            OsString::from("gruvbox-dark-hard"),
        ],
        None,
    );
    assert!(theme.status.success(), "theme stderr: {:?}", theme.stderr);
    let relays = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("relays"),
            OsString::from("wss://z.example"),
            OsString::from("wss://a.example"),
        ],
        None,
    );
    assert!(
        relays.status.success(),
        "relays stderr: {:?}",
        relays.stderr
    );
    let relays: serde_json::Value =
        serde_json::from_slice(&relays.stdout).expect("configuration JSON");
    assert_eq!(relays["kind"], "configuration");
    assert_eq!(relays["data"]["default_provider"], "codex");
    assert_eq!(relays["data"]["theme"], "gruvbox-dark-hard");
    assert_eq!(
        relays["data"]["relays"],
        serde_json::json!(["wss://a.example", "wss://z.example"])
    );

    let duplicate = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("relays"),
            OsString::from("wss://a.example"),
            OsString::from("wss://a.example"),
        ],
        None,
    );
    assert!(!duplicate.status.success());
    let preserved = offline_output(
        &state_root,
        [OsString::from("config"), OsString::from("get")],
        None,
    );
    let preserved: serde_json::Value =
        serde_json::from_slice(&preserved.stdout).expect("preserved configuration JSON");
    assert_eq!(preserved["data"], relays["data"]);

    let themes = offline_output(
        &state_root,
        [OsString::from("config"), OsString::from("themes")],
        None,
    );
    assert!(
        themes.status.success(),
        "themes stderr: {:?}",
        themes.stderr
    );
    let themes: serde_json::Value =
        serde_json::from_slice(&themes.stdout).expect("theme catalog JSON");
    assert_eq!(themes["kind"], "themes");
    assert!(
        themes["data"]["themes"]
            .as_array()
            .expect("theme entries")
            .iter()
            .any(|entry| entry["selector"] == "gruvbox-dark-hard" && entry["active"] == true)
    );

    let missing = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("theme"),
            OsString::from("missing-theme"),
        ],
        None,
    );
    assert!(!missing.status.success());
    let after_missing = offline_output(
        &state_root,
        [OsString::from("config"), OsString::from("get")],
        None,
    );
    let after_missing: serde_json::Value =
        serde_json::from_slice(&after_missing.stdout).expect("preserved theme selection");
    assert_eq!(after_missing["data"]["theme"], "gruvbox-dark-hard");

    let cleared = offline_output(
        &state_root,
        [
            OsString::from("config"),
            OsString::from("set"),
            OsString::from("theme"),
            OsString::from("none"),
        ],
        None,
    );
    assert!(
        cleared.status.success(),
        "clear stderr: {:?}",
        cleared.stderr
    );
    let cleared: serde_json::Value =
        serde_json::from_slice(&cleared.stdout).expect("cleared configuration JSON");
    assert_eq!(cleared["data"]["theme"], serde_json::Value::Null);

    let paths = StatePaths::new(state_root.clone()).expect("state paths");
    let live_owner = StateDirectoryOwner::acquire(paths).expect("test owns state");
    let refused = offline_output(
        &state_root,
        [OsString::from("identity"), OsString::from("show")],
        None,
    );
    assert!(!refused.status.success());
    drop(live_owner);
}

#[test]
fn human_account_creation_reconciles_concurrent_callers_and_survives_restart() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let identity = initialize_identity(&state_root);
    let installation = identity["data"]["installation_id"]
        .as_str()
        .expect("installation id")
        .to_owned();

    let first_root = state_root.clone();
    let second_root = state_root.clone();
    let first = std::thread::spawn(move || human_output(&first_root, &["create", "Personal"]));
    let second = std::thread::spawn(move || human_output(&second_root, &["create", "Personal"]));
    let first = first.join().expect("first creator");
    let second = second.join().expect("second creator");
    assert!(first.status.success(), "first stderr: {:?}", first.stderr);
    assert!(
        second.status.success(),
        "second stderr: {:?}",
        second.stderr
    );
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).expect("first human JSON");
    let second: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("second human JSON");
    assert_eq!(first["data"], second["data"]);
    let account = first["data"]["active_account"]
        .as_str()
        .expect("active account");
    assert_ne!(account, installation);
    assert_eq!(first["data"]["accounts"].as_array().map(Vec::len), Some(1));
    assert_eq!(first["data"]["accounts"][0]["account_id"], account);
    assert_eq!(
        first["data"]["accounts"][0]["creator_installation"],
        installation
    );
    assert_eq!(first["data"]["accounts"][0]["label"], "Personal");
    assert_eq!(first["data"]["accounts"][0]["selected"], true);

    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let mut client = local_client(state, InitialView::OnDemand);
    assert_eq!(
        client.snapshot().expect("authoritative snapshot").revision,
        4,
        "installation, mailbox, account, and selection are each authored exactly once"
    );

    let repeated = human_output(&state_root, &["create", "Personal"]);
    assert!(repeated.status.success());
    assert_eq!(
        client
            .snapshot()
            .expect("snapshot after reconcile")
            .revision,
        4
    );

    let changed_label = human_output(&state_root, &["create", "Work"]);
    assert!(!changed_label.status.success());
    let changed_error: serde_json::Value =
        serde_json::from_slice(&changed_label.stderr).expect("changed-label error JSON");
    assert_eq!(changed_error["data"]["code"], "human.state_unavailable");

    let unknown_id = "11".repeat(32);
    let unknown = human_output(&state_root, &["select", &unknown_id]);
    assert!(!unknown.status.success());
    assert_eq!(
        client.snapshot().expect("snapshot after refusals").revision,
        4
    );

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let shown = human_output(&state_root, &["show"]);
    assert!(shown.status.success(), "show stderr: {:?}", shown.stderr);
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).expect("shown human JSON");
    assert_eq!(shown["data"], first["data"]);

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn human_pairing_is_target_bound_replay_safe_and_survives_restart() {
    let directory = TestDirectory::new();
    let creator_root = directory.path().join("creator");
    let device_root = directory.path().join("device");
    let _creator_identity = initialize_identity(&creator_root);
    let device_identity = initialize_identity(&device_root);
    let device_id = device_identity["data"]["installation_id"]
        .as_str()
        .expect("device installation");
    let device_key = device_identity["data"]["signing_public_key"]
        .as_str()
        .expect("device signing key");
    let created = human_output(&creator_root, &["create", "Personal"]);
    assert!(
        created.status.success(),
        "create stderr: {:?}",
        created.stderr
    );
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).expect("creator JSON");
    let account = created["data"]["active_account"]
        .as_str()
        .expect("creator account")
        .to_owned();
    let invitation = directory.path().join("pairing-invitation.json");

    let invited = offline_output(
        &creator_root,
        [
            OsString::from("human"),
            OsString::from("invite"),
            OsString::from(device_id),
            OsString::from(device_key),
            invitation.clone().into_os_string(),
            OsString::from("--label"),
            OsString::from("laptop"),
            OsString::from("--relay"),
            OsString::from("wss://relay.example"),
        ],
        None,
    );
    assert!(
        invited.status.success(),
        "invite stderr: {:?}",
        invited.stderr
    );
    let invited_json: serde_json::Value =
        serde_json::from_slice(&invited.stdout).expect("invite JSON");
    assert_eq!(invited_json["kind"], "human_pairing");
    assert_eq!(invited_json["data"]["operation"], "invite");
    assert_eq!(invited_json["data"]["account_id"], account);
    assert_eq!(invited_json["data"]["device"], device_id);
    assert!(
        !String::from_utf8_lossy(&invited.stdout).contains(&invitation.display().to_string()),
        "output must not disclose the caller-selected path"
    );

    let creator_state = StatePaths::new(creator_root.clone()).expect("creator state");
    let mut creator_client = local_client(creator_state, InitialView::OnDemand);
    let invite_revision = creator_client.snapshot().expect("invite snapshot").revision;
    let repeated_invitation = directory.path().join("repeated-pairing-invitation.json");
    let repeated_invite = offline_output(
        &creator_root,
        [
            OsString::from("human"),
            OsString::from("invite"),
            OsString::from(device_id),
            OsString::from(device_key),
            repeated_invitation.clone().into_os_string(),
            OsString::from("--label"),
            OsString::from("laptop"),
            OsString::from("--relay"),
            OsString::from("wss://relay.example"),
        ],
        None,
    );
    assert!(
        repeated_invite.status.success(),
        "repeat invite stderr: {:?}",
        repeated_invite.stderr
    );
    assert_eq!(
        creator_client
            .snapshot()
            .expect("snapshot after repeat invite")
            .revision,
        invite_revision,
        "an unrevoked current grant is reused"
    );
    assert_eq!(
        fs::read(&repeated_invitation).expect("repeated invitation reads"),
        fs::read(&invitation).expect("original invitation reads")
    );

    let wrong_target = human_output(
        &creator_root,
        &["join", invitation.to_str().expect("UTF-8 path")],
    );
    assert!(!wrong_target.status.success());
    let wrong_error: serde_json::Value =
        serde_json::from_slice(&wrong_target.stderr).expect("wrong-target error JSON");
    assert_eq!(wrong_error["data"]["code"], "human.pairing_invalid");

    let joined = human_output(
        &device_root,
        &["join", invitation.to_str().expect("UTF-8 path")],
    );
    assert!(joined.status.success(), "join stderr: {:?}", joined.stderr);
    let joined_json: serde_json::Value = serde_json::from_slice(&joined.stdout).expect("join JSON");
    assert_eq!(joined_json["data"]["operation"], "join");
    assert_eq!(joined_json["data"]["account_id"], account);
    assert_eq!(joined_json["data"]["device"], device_id);

    let device_state = StatePaths::new(device_root.clone()).expect("device state");
    let mut device_client = local_client(device_state, InitialView::OnDemand);
    let joined_revision = device_client.snapshot().expect("joined snapshot").revision;
    let repeated = human_output(
        &device_root,
        &["join", invitation.to_str().expect("UTF-8 path")],
    );
    assert!(
        repeated.status.success(),
        "repeat join stderr: {:?}",
        repeated.stderr
    );
    assert_eq!(
        device_client
            .snapshot()
            .expect("snapshot after repeated join")
            .revision,
        joined_revision,
        "byte-identical evidence, acceptance, and selection are no-ops"
    );

    let device_listing = human_output(&device_root, &["devices"]);
    assert!(
        device_listing.status.success(),
        "device listing stderr: {:?}",
        device_listing.stderr
    );
    let device_listing: serde_json::Value =
        serde_json::from_slice(&device_listing.stdout).expect("device listing JSON");
    assert_eq!(device_listing["kind"], "human_devices");
    let joined_device = device_listing["data"]["devices"]
        .as_array()
        .expect("devices array")
        .iter()
        .find(|device| device["installation_id"] == device_id)
        .expect("joined device");
    assert_eq!(joined_device["state"], "active");
    assert_eq!(
        joined_device["acceptances"]
            .as_array()
            .expect("acceptances")
            .len(),
        1
    );

    let non_creator_revoke = human_output(&device_root, &["revoke", device_id]);
    assert!(!non_creator_revoke.status.success());
    let non_creator_error: serde_json::Value =
        serde_json::from_slice(&non_creator_revoke.stderr).expect("non-creator error JSON");
    assert_eq!(non_creator_error["data"]["code"], "human.state_unavailable");

    let before_revoke = creator_client
        .snapshot()
        .expect("snapshot before revoke")
        .revision;
    let revoked = human_output(&creator_root, &["revoke", device_id]);
    assert!(
        revoked.status.success(),
        "creator revoke stderr: {:?}",
        revoked.stderr
    );
    let revoked_json: serde_json::Value =
        serde_json::from_slice(&revoked.stdout).expect("revoked device JSON");
    let revoked_device = revoked_json["data"]["devices"]
        .as_array()
        .expect("devices array")
        .iter()
        .find(|device| device["installation_id"] == device_id)
        .expect("revoked device");
    assert_eq!(revoked_device["state"], "revoked");
    assert_eq!(
        revoked_device["revokes"].as_array().expect("revokes").len(),
        1
    );
    let revoked_revision = creator_client
        .snapshot()
        .expect("snapshot after revoke")
        .revision;
    assert_eq!(revoked_revision, before_revoke + 1);
    let repeated_revoke = human_output(&creator_root, &["revoke", device_id]);
    assert!(
        repeated_revoke.status.success(),
        "repeat revoke stderr: {:?}",
        repeated_revoke.stderr
    );
    assert_eq!(
        creator_client
            .snapshot()
            .expect("snapshot after repeat revoke")
            .revision,
        revoked_revision,
        "repeat revoke is a semantic no-op"
    );

    let tampered_path = directory.path().join("tampered-invitation.json");
    let mut tampered = fs::read(&invitation).expect("invitation reads");
    let byte = tampered
        .iter_mut()
        .find(|byte| **byte == b'a')
        .expect("fixture contains a mutable byte");
    *byte = b'b';
    fs::write(&tampered_path, tampered).expect("tampered fixture writes");
    let rejected = human_output(
        &device_root,
        &["join", tampered_path.to_str().expect("UTF-8 path")],
    );
    assert!(!rejected.status.success());
    assert_eq!(
        device_client
            .snapshot()
            .expect("snapshot after tamper")
            .revision,
        joined_revision
    );

    let restarted = output("restart", &device_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let shown = human_output(&device_root, &["show"]);
    assert!(shown.status.success(), "show stderr: {:?}", shown.stderr);
    let shown: serde_json::Value =
        serde_json::from_slice(&shown.stdout).expect("joined human JSON");
    assert_eq!(shown["data"]["active_account"], account);

    let creator_restarted = output("restart", &creator_root);
    assert!(
        creator_restarted.status.success(),
        "creator restart stderr: {:?}",
        creator_restarted.stderr
    );
    let persisted = human_output(&creator_root, &["devices"]);
    assert!(
        persisted.status.success(),
        "persisted devices stderr: {:?}",
        persisted.stderr
    );
    let persisted: serde_json::Value =
        serde_json::from_slice(&persisted.stdout).expect("persisted device JSON");
    assert!(
        persisted["data"]["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["installation_id"] == device_id && device["state"] == "revoked")
    );

    for root in [&creator_root, &device_root] {
        let stopped = output("stop", root);
        assert!(
            stopped.status.success(),
            "stop stderr: {:?}",
            stopped.stderr
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn directional_peer_and_mailbox_authority_is_replay_safe_and_recovers_after_distrust() {
    let directory = TestDirectory::new();
    let owner_root = directory.path().join("owner");
    let peer_root = directory.path().join("peer");
    let owner_identity = initialize_identity(&owner_root);
    let peer_identity = initialize_identity(&peer_root);
    let owner_id = owner_identity["data"]["installation_id"]
        .as_str()
        .expect("owner installation");
    let peer_id = peer_identity["data"]["installation_id"]
        .as_str()
        .expect("peer installation");
    let peer_signing_key = peer_identity["data"]["signing_public_key"]
        .as_str()
        .expect("peer signing key");

    let peer_state = StatePaths::new(peer_root.clone()).expect("peer state");
    let mut peer_client = local_client(peer_state, InitialView::OnDemand);
    let peer_snapshot = peer_client.snapshot().expect("peer snapshot");
    let peer_encryption_key = peer_snapshot
        .items
        .iter()
        .find_map(|item| match item {
            SnapshotItem::Installation { encryption_key, .. } => {
                Some(encode_hex(encryption_key.bytes()))
            }
            _ => None,
        })
        .expect("peer encryption key");

    let created = human_output(&owner_root, &["create", "Personal"]);
    assert!(
        created.status.success(),
        "create stderr: {:?}",
        created.stderr
    );
    let mailboxes = admin_output(&owner_root, "mailbox", &["list"]);
    assert!(
        mailboxes.status.success(),
        "mailbox list stderr: {:?}",
        mailboxes.stderr
    );
    let mailboxes: serde_json::Value =
        serde_json::from_slice(&mailboxes.stdout).expect("mailbox JSON");
    let mailbox_id = mailboxes["data"]["mailboxes"][0]["mailbox_id"]
        .as_str()
        .expect("local mailbox");

    let added = admin_output(
        &owner_root,
        "peer",
        &[
            "add",
            peer_id,
            peer_signing_key,
            &peer_encryption_key,
            "--label",
            "collaborator",
            "--relay",
            "wss://relay.example",
        ],
    );
    assert!(
        added.status.success(),
        "peer add stderr: {:?}",
        added.stderr
    );
    let added: serde_json::Value = serde_json::from_slice(&added.stdout).expect("peer JSON");
    assert_eq!(added["kind"], "authority_admin");
    assert_eq!(added["data"]["peers"][0]["peer"], peer_id);
    assert_eq!(added["data"]["peers"][0]["state"], "routable");
    assert_eq!(
        added["data"]["peers"][0]["routes"][0]["relay_hints"][0]["value"],
        "wss://relay.example"
    );

    let owner_state = StatePaths::new(owner_root.clone()).expect("owner state");
    let mut owner_client = local_client(owner_state, InitialView::OnDemand);
    let added_revision = owner_client.snapshot().expect("added snapshot").revision;
    let repeated_add = admin_output(
        &owner_root,
        "peer",
        &[
            "add",
            peer_id,
            peer_signing_key,
            &peer_encryption_key,
            "--label",
            "collaborator",
            "--relay",
            "wss://relay.example",
        ],
    );
    assert!(repeated_add.status.success());
    assert_eq!(
        owner_client
            .snapshot()
            .expect("repeated add snapshot")
            .revision,
        added_revision,
        "an exact current route is reused"
    );

    let granted = admin_output(&owner_root, "mailbox", &["grant", mailbox_id, peer_id]);
    assert!(
        granted.status.success(),
        "mailbox grant stderr: {:?}",
        granted.stderr
    );
    let granted: serde_json::Value = serde_json::from_slice(&granted.stdout).expect("grant JSON");
    assert_eq!(granted["data"]["capabilities"][0]["active"], true);
    assert_eq!(
        granted["data"]["capabilities"][0]["grantee_installation"],
        peer_id
    );
    assert_eq!(
        granted["data"]["capabilities"][0]["grantee_signing_key"],
        peer_signing_key
    );
    let granted_revision = owner_client.snapshot().expect("grant snapshot").revision;
    let repeated_grant = admin_output(&owner_root, "mailbox", &["grant", mailbox_id, peer_id]);
    assert!(repeated_grant.status.success());
    assert_eq!(
        owner_client
            .snapshot()
            .expect("repeated grant snapshot")
            .revision,
        granted_revision,
        "an exact active capability is reused"
    );

    let distrusted = admin_output(&owner_root, "peer", &["distrust", peer_id]);
    assert!(
        distrusted.status.success(),
        "distrust stderr: {:?}",
        distrusted.stderr
    );
    let distrusted: serde_json::Value =
        serde_json::from_slice(&distrusted.stdout).expect("distrust JSON");
    assert_eq!(distrusted["data"]["peers"][0]["state"], "blocked");
    assert_eq!(distrusted["data"]["capabilities"][0]["active"], false);
    let distrusted_revision = owner_client.snapshot().expect("distrust snapshot").revision;
    assert_eq!(
        distrusted_revision,
        granted_revision + 2,
        "distrust revokes the capability before authoring the route block"
    );
    let repeated_distrust = admin_output(&owner_root, "peer", &["distrust", peer_id]);
    assert!(repeated_distrust.status.success());
    assert_eq!(
        owner_client
            .snapshot()
            .expect("repeated distrust snapshot")
            .revision,
        distrusted_revision
    );

    let recovered = admin_output(
        &owner_root,
        "peer",
        &[
            "add",
            peer_id,
            peer_signing_key,
            &peer_encryption_key,
            "--label",
            "collaborator",
            "--relay",
            "wss://relay.example",
        ],
    );
    assert!(
        recovered.status.success(),
        "recovery stderr: {:?}",
        recovered.stderr
    );
    let recovered: serde_json::Value =
        serde_json::from_slice(&recovered.stdout).expect("recovery JSON");
    assert_eq!(recovered["data"]["peers"][0]["state"], "routable");
    assert_eq!(
        recovered["data"]["peers"][0]["blocks"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        recovered["data"]["peers"][0]["routes"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let regranted = admin_output(&owner_root, "mailbox", &["grant", mailbox_id, peer_id]);
    assert!(
        regranted.status.success(),
        "regrant stderr: {:?}",
        regranted.stderr
    );
    let regranted: serde_json::Value =
        serde_json::from_slice(&regranted.stdout).expect("regrant JSON");
    let capabilities = regranted["data"]["capabilities"]
        .as_array()
        .expect("capabilities");
    assert_eq!(capabilities.len(), 2);
    assert_eq!(
        capabilities
            .iter()
            .filter(|capability| capability["active"] == true)
            .count(),
        1
    );

    let unauthorized = admin_output(&peer_root, "mailbox", &["grant", mailbox_id, owner_id]);
    assert!(!unauthorized.status.success());
    let unauthorized: serde_json::Value =
        serde_json::from_slice(&unauthorized.stderr).expect("authority error JSON");
    assert_eq!(unauthorized["data"]["code"], "authority.state_unavailable");

    let restarted = output("restart", &owner_root);
    assert!(
        restarted.status.success(),
        "owner restart stderr: {:?}",
        restarted.stderr
    );
    let persisted = admin_output(&owner_root, "mailbox", &["list"]);
    assert!(persisted.status.success());
    let persisted: serde_json::Value =
        serde_json::from_slice(&persisted.stdout).expect("persisted authority JSON");
    assert_eq!(
        persisted["data"]["capabilities"]
            .as_array()
            .expect("persisted capabilities")
            .iter()
            .filter(|capability| capability["active"] == true)
            .count(),
        1
    );

    for root in [&owner_root, &peer_root] {
        let stopped = output("stop", root);
        assert!(
            stopped.status.success(),
            "stop stderr: {:?}",
            stopped.stderr
        );
    }
}

#[test]
fn relay_administration_is_idempotent_redacted_and_restart_durable() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let _identity = initialize_identity(&state_root);
    let endpoint = "ws://127.0.0.1:9";

    let initial = admin_json(&state_root, "relay", &["status"]);
    assert_eq!(initial["kind"], "relay_admin");
    assert_eq!(initial["data"]["policies"], serde_json::json!([]));
    assert_eq!(initial["data"]["domains"].as_array().map(Vec::len), Some(4));

    let added = admin_json(
        &state_root,
        "relay",
        &["add", endpoint, "--access", "read", "--auth", "required"],
    );
    assert_eq!(added["data"]["outcome"], "accepted");
    assert_eq!(
        added["data"]["operation_id"].as_str().map(str::len),
        Some(64)
    );
    assert_eq!(added["data"]["policies"][0]["endpoint"], endpoint);
    assert_eq!(added["data"]["policies"][0]["access"], "read");
    assert_eq!(added["data"]["policies"][0]["authentication"], "required");
    assert_eq!(added["data"]["policies"][0]["enabled"], true);
    assert_eq!(added["data"]["policies"][0]["generation"], 1);

    let repeated = admin_json(
        &state_root,
        "relay",
        &["add", endpoint, "--access", "read", "--auth", "required"],
    );
    assert_eq!(repeated["data"]["outcome"], "unchanged");
    assert_eq!(repeated["data"]["policies"][0]["generation"], 1);

    let synchronized = admin_json(&state_root, "relay", &["sync", endpoint]);
    assert_eq!(synchronized["data"]["outcome"], "accepted");

    let repaired = admin_json(&state_root, "relay", &["repair"]);
    assert_eq!(repaired["data"]["outcome"], "repaired");
    assert_eq!(
        repaired["data"]["operation_id"].as_str().map(str::len),
        Some(64)
    );

    let removed = admin_json(&state_root, "relay", &["remove", endpoint]);
    assert_eq!(removed["data"]["policies"][0]["enabled"], false);
    assert_eq!(removed["data"]["policies"][0]["generation"], 2);

    let disabled_sync = admin_json(&state_root, "relay", &["sync", endpoint]);
    assert_eq!(disabled_sync["data"]["outcome"], "rejected");
    assert_eq!(
        disabled_sync["data"]["operation_id"].as_str().map(str::len),
        Some(64)
    );

    let repeated_remove = admin_json(&state_root, "relay", &["remove", endpoint]);
    assert_eq!(repeated_remove["data"]["outcome"], "unchanged");
    assert_eq!(repeated_remove["data"]["policies"][0]["generation"], 2);

    let restarted = output("restart", &state_root);
    assert!(
        restarted.status.success(),
        "restart stderr: {:?}",
        restarted.stderr
    );
    let persisted = admin_json(&state_root, "relay", &["list"]);
    assert_eq!(persisted["data"]["policies"][0]["endpoint"], endpoint);
    assert_eq!(persisted["data"]["policies"][0]["enabled"], false);
    assert_eq!(persisted["data"]["policies"][0]["generation"], 2);

    let secret_endpoint = "ws://secret@127.0.0.1:9";
    let invalid = admin_output(&state_root, "relay", &["add", secret_endpoint]);
    assert!(!invalid.status.success());
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains(secret_endpoint));

    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
}

#[test]
fn startup_refuses_a_persisted_root_that_disagrees_with_the_owned_identity() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let other_root = directory.path().join("other");
    for root in [&state_root, &other_root] {
        let initialized = offline_output(
            root,
            [OsString::from("identity"), OsString::from("init")],
            None,
        );
        assert!(
            initialized.status.success(),
            "init stderr: {:?}",
            initialized.stderr
        );
    }

    let ready = output("readiness", &state_root);
    assert!(
        ready.status.success(),
        "readiness stderr: {:?}",
        ready.stderr
    );
    let stopped = output("stop", &state_root);
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );

    let state = StatePaths::new(state_root.clone()).expect("state paths");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(owner) = StateDirectoryOwner::acquire(state.clone()) {
            drop(owner);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stopped node did not release state ownership"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let other = StatePaths::new(other_root).expect("other state paths");
    let replacement = fs::read(other.identity_file()).expect("other identity reads");
    fs::write(state.identity_file(), replacement).expect("identity fixture is replaced");

    let mismatch = command("run", &state_root)
        .stdin(Stdio::null())
        .output()
        .expect("mismatched foreground exits");
    assert!(!mismatch.status.success());
    let stderr = String::from_utf8_lossy(&mismatch.stderr);
    assert!(stderr.contains("node.foreground_failed"));
    assert!(!stderr.contains("signing"));
    assert!(!stderr.contains("secret"));
    let owner = StateDirectoryOwner::acquire(state).expect("failed startup releases ownership");
    drop(owner);
}

#[test]
fn process_owning_test_directories_serialize_across_test_threads() {
    let first = TestDirectory::new();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
    let contender = std::thread::spawn(move || {
        started_tx.send(()).expect("start signal");
        let second = TestDirectory::new();
        acquired_tx.send(()).expect("acquired signal");
        drop(second);
    });
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("contender starts");
    assert!(
        acquired_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "another process-owning fixture must wait"
    );
    drop(first);
    acquired_rx
        .recv_timeout(Duration::from_secs(120))
        .expect("contender acquires after release");
    contender.join().expect("contender exits");
}

#[test]
fn concurrent_readiness_callers_spawn_candidates_but_converge_on_one_owner() {
    let directory = TestDirectory::new();
    let (state, runtime) = initialize(&directory);
    let first_root = state.root().to_path_buf();
    let second_root = first_root.clone();
    let first = std::thread::spawn(move || output("readiness", &first_root));
    let second = std::thread::spawn(move || output("readiness", &second_root));
    let first = first.join().expect("first caller");
    let second = second.join().expect("second caller");
    assert!(first.status.success(), "first stderr: {:?}", first.stderr);
    assert!(
        second.status.success(),
        "second stderr: {:?}",
        second.stderr
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("readiness=ready"));
    assert!(String::from_utf8_lossy(&second.stdout).contains("readiness=ready"));

    let stopped = output("stop", state.root());
    assert!(
        stopped.status.success(),
        "stop stderr: {:?}",
        stopped.stderr
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.socket_file().exists() || runtime.readiness_file().exists() {
        assert!(
            Instant::now() < deadline,
            "autostart artifacts did not clean"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
