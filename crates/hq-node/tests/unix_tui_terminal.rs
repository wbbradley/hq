//! Installed terminal routing and pseudoterminal restoration contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    os::fd::OwnedFd,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    pty::{Winsize, openpty},
    sys::termios::{Termios, tcgetattr},
};

use support::TestDirectory;

const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h";
const LEAVE_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049l";
const AUTHORITATIVE_STATE_PROBE_INTERVAL: Duration = Duration::from_millis(50);
// This is an inactivity watchdog for installed process tests, not a product latency budget.
const PROCESS_INACTIVITY_WATCHDOG: Duration = Duration::from_secs(30);
static PSEUDOTERMINAL_SCENARIO: Mutex<()> = Mutex::new(());

#[test]
fn installed_tui_without_an_identity_fails_fast_before_terminal_activation() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");

    for explicit in [true, false] {
        let started = Instant::now();
        let run = run_in_pty(&state_root, explicit, PtyInteraction::QuitOnStart);
        assert!(!run.status.success(), "uninitialized TUI unexpectedly ran");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "identity preflight did not fail fast: {:?}",
            started.elapsed()
        );
        assert!(
            run.bytes
                .windows(b"setup.identity_required".len())
                .any(|window| window == b"setup.identity_required"),
            "missing typed setup diagnostic: {:?}",
            run.bytes
        );
        assert!(
            run.bytes
                .windows(b"hq identity init".len())
                .any(|window| window == b"hq identity init"),
            "missing identity setup action: {:?}",
            run.bytes
        );
        for explanation in [
            "HQ needs a device identity before it can protect your account and messages.",
            "Then run `hq` again; the next screen will guide account setup.",
        ] {
            assert!(
                run.bytes
                    .windows(explanation.len())
                    .any(|window| window == explanation.as_bytes()),
                "missing first-run explanation {explanation:?}: {:?}",
                run.bytes
            );
        }
        assert!(
            !run.bytes
                .windows(ENTER_ALTERNATE_SCREEN.len())
                .any(|window| window == ENTER_ALTERNATE_SCREEN),
            "identity preflight activated the terminal"
        );
        assert_eq!(run.before, run.after, "preflight changed terminal modes");
    }
}

#[test]
fn setup_commands_leave_the_next_action_on_screen() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let identity = hq_output(&state_root, &["identity", "init"]);
    assert!(
        identity.status.success(),
        "identity init failed: {identity:?}"
    );
    assert!(
        String::from_utf8_lossy(&identity.stdout).contains("Next: run hq"),
        "identity setup omitted its next action: {:?}",
        identity.stdout
    );
    let _daemon = DaemonStopGuard(&state_root);
    let human = hq_output(&state_root, &["human", "create"]);
    assert!(human.status.success(), "human create failed: {human:?}");
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("Next: run hq"),
        "account setup omitted its next action: {:?}",
        human.stdout
    );
}

#[test]
fn fresh_account_setup_continues_into_guided_work_across_restart() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);

    let run = run_in_pty(
        &state_root,
        false,
        PtyInteraction::CompleteFreshSetupAndReconnect,
    );
    assert!(
        run.status.success(),
        "fresh walkthrough failed: {:?}",
        run.bytes
    );
    for phrase in [
        "No human account is selected",
        "Get started with HQ",
        "add a project",
        "Work with an agent on a project",
        "Help for New",
    ] {
        assert!(
            run.bytes
                .windows(phrase.len())
                .any(|window| window == phrase.as_bytes()),
            "fresh walkthrough omitted {phrase:?}: {:?}",
            run.bytes
        );
    }
    assert_eq!(
        run.before, run.after,
        "fresh walkthrough changed terminal modes"
    );
}

#[test]
fn explicit_and_bare_tui_render_and_restore_the_pseudoterminal() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);

    for explicit in [true, false] {
        let run = run_in_pty(&state_root, explicit, PtyInteraction::QuitAfterSetup);
        assert!(run.status.success(), "TUI process failed: {:?}", run.bytes);
        assert!(
            run.bytes
                .windows(ENTER_ALTERNATE_SCREEN.len())
                .any(|window| window == ENTER_ALTERNATE_SCREEN),
            "TUI did not enter the alternate screen"
        );
        assert!(
            run.bytes.windows(2).any(|window| window == b"HQ"),
            "TUI did not render its title"
        );
        assert!(
            run.bytes
                .windows(b"No human account is selected".len())
                .any(|window| window == b"No human account is selected"),
            "identity-only TUI did not render setup and recovery guidance"
        );
        assert!(
            run.bytes
                .windows(LEAVE_ALTERNATE_SCREEN.len())
                .any(|window| window == LEAVE_ALTERNATE_SCREEN),
            "TUI did not leave the alternate screen"
        );
        assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
    }
}

#[test]
fn installed_tui_self_note_matches_cli_and_survives_restart() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    let human = hq_output(&state_root, &["human", "create"]);
    assert!(
        human.status.success(),
        "human create failed: {:?}",
        human.stderr
    );

    let content = "installed tui durable note";
    let run = run_in_pty(&state_root, true, PtyInteraction::SubmitSelfNote(content));
    assert!(run.status.success(), "TUI process failed: {:?}", run.bytes);
    let listing = hq_output(&state_root, &["--output", "json", "list", "--all"]);
    assert!(
        listing.status.success() && String::from_utf8_lossy(&listing.stdout).contains(content),
        "CLI listing did not contain TUI note: status={:?} stdout={:?} stderr={:?}",
        listing.status,
        listing.stdout,
        listing.stderr
    );
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");

    let restarted = hq_output(&state_root, &["daemon", "restart"]);
    assert!(
        restarted.status.success(),
        "daemon restart failed: {:?}",
        restarted.stderr
    );
    assert!(mailbox_contains(&state_root, content));
}

#[test]
fn installed_inbox_eagerly_renders_and_returns_from_conversation_to_its_list() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    assert!(
        hq_output(&state_root, &["human", "create"])
            .status
            .success()
    );

    let content = "[installed inbox](https://example.test/preview)";
    let seeded = run_in_pty(&state_root, true, PtyInteraction::SubmitSelfNote(content));
    assert!(
        seeded.status.success(),
        "note setup failed: {:?}",
        seeded.bytes
    );

    let run = run_in_pty(
        &state_root,
        true,
        PtyInteraction::NavigateInboxConversation(content),
    );
    assert!(
        run.status.success(),
        "Inbox navigation failed: {:?}",
        run.bytes
    );
    for phrase in [
        "Personal notes",
        content,
        "installed inbox",
        "https://example.test/preview",
        "You",
        "h/← Inbox",
        "Enter open",
    ] {
        assert!(
            run.bytes
                .windows(phrase.len())
                .any(|window| window == phrase.as_bytes()),
            "Inbox navigation omitted {phrase:?}: {:?}",
            run.bytes
        );
    }
    for obsolete in [
        "Conversation · complete",
        "message · open",
        "update · information only",
    ] {
        assert!(
            !run.bytes
                .windows(obsolete.len())
                .any(|window| window == obsolete.as_bytes()),
            "Inbox navigation retained {obsolete:?}: {:?}",
            run.bytes
        );
    }
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
}

#[test]
fn installed_markdown_content_is_inert_and_resource_free() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let absent_image = directory.path().join("renderer-must-not-read-this.png");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    assert!(
        hq_output(&state_root, &["human", "create"])
            .status
            .success()
    );

    let wide_cell = "x".repeat(240);
    let content = format!(
        concat!(
            "# Adversarial Markdown\n\n",
            "\x1b[999zCSI_MARKER \x1b]777;OSC_MARKER\x07 C1\u{0085} DEL\u{007f}\n\n",
            "<span>raw HTML</span> [safe link](https://example.test/inert) ",
            "![remote image](https://192.0.2.1/never-load.png)\n\n",
            "```text\n\x1b[888zCODE_MARKER\n```\n\n",
            "- outer item\n  - nested item with continuation words\n\n",
            "| Name | Oversized value |\n| --- | --- |\n| bounded | {} |\n\n",
            "![absent local image](file://{})",
        ),
        wide_cell,
        absent_image.display()
    );
    let seeded = run_in_pty(
        &state_root,
        true,
        PtyInteraction::SubmitPastedSelfNote {
            content: &content,
            marker: "Adversarial Markdown",
        },
    );
    assert!(
        seeded.status.success(),
        "adversarial note setup failed: {:?}",
        seeded.bytes
    );
    assert!(!absent_image.exists());

    let run = run_in_pty(
        &state_root,
        true,
        PtyInteraction::NavigateInboxConversation("Adversarial Markdown"),
    );
    assert!(
        run.status.success(),
        "adversarial Inbox navigation failed: {:?}",
        run.bytes
    );
    for phrase in [
        "Adversarial Markdown",
        "CSI_MARKER",
        "OSC_MARKER",
        "raw HTML",
        "https://example.test/inert",
        "https://192.0.2.1/never-load.png",
        "CODE_MARKER",
        "nested item",
        "h/← Inbox",
    ] {
        assert!(
            run.bytes
                .windows(phrase.len())
                .any(|window| window == phrase.as_bytes()),
            "installed Markdown omitted {phrase:?}: {:?}",
            run.bytes
        );
    }
    for injection in [
        b"\x1b[999zCSI_MARKER".as_slice(),
        b"\x1b]777;OSC_MARKER\x07".as_slice(),
        b"\x1b[888zCODE_MARKER".as_slice(),
    ] {
        assert!(
            !seeded
                .bytes
                .windows(injection.len())
                .any(|window| window == injection)
        );
        assert!(
            !run.bytes
                .windows(injection.len())
                .any(|window| window == injection)
        );
    }
    assert!(!absent_image.exists());
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
}

#[test]
fn installed_tui_new_launcher_explains_all_three_intents() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    assert!(
        hq_output(&state_root, &["human", "create"])
            .status
            .success()
    );

    let run = run_in_pty(&state_root, true, PtyInteraction::OpenNewLauncher);
    assert!(run.status.success(), "TUI process failed: {:?}", run.bytes);
    for phrase in [
        "Work with an agent on a project",
        "direct",
        "message:",
        "Write a personal",
        "yourself",
    ] {
        assert!(
            run.bytes
                .windows(phrase.len())
                .any(|window| window == phrase.as_bytes()),
            "New launcher omitted {phrase:?}: {:?}",
            run.bytes
        );
    }
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
}

#[test]
fn installed_tui_agent_create_matches_cli_and_survives_restart() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    let human = hq_output(&state_root, &["human", "create"]);
    assert!(
        human.status.success(),
        "human create failed: {:?}",
        human.stderr
    );

    let name = "tui-builder";
    let run = run_in_pty(&state_root, true, PtyInteraction::CreateAgent(name));
    assert!(run.status.success(), "TUI process failed: {:?}", run.bytes);
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
    assert!(agent_exists(&state_root, name));

    let restarted = hq_output(&state_root, &["daemon", "restart"]);
    assert!(
        restarted.status.success(),
        "daemon restart failed: {:?}",
        restarted.stderr
    );
    assert!(agent_exists(&state_root, name));
}

#[test]
fn installed_tui_automatically_uses_the_available_provider_and_renders_typed_failure() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    let human = hq_output(&state_root, &["human", "create"]);
    assert!(
        human.status.success(),
        "human create failed: {:?}",
        human.stderr
    );
    let created = hq_output(
        &state_root,
        &["--output", "json", "agent", "create", "runtime-agent"],
    );
    assert!(
        created.status.success(),
        "agent create failed: {:?}",
        created.stderr
    );

    let run = run_in_pty(&state_root, true, PtyInteraction::StartRejectedSession);
    assert!(run.status.success(), "TUI process failed: {:?}", run.bytes);
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
    assert!(
        run.bytes
            .windows(b"HQ could not confirm whether the change finished.".len())
            .any(|window| window == b"HQ could not confirm whether the change finished."),
        "TUI did not render the typed provider failure: {:?}",
        run.bytes
    );
    assert!(agent_exists(&state_root, "runtime-agent"));

    let restarted = hq_output(&state_root, &["daemon", "restart"]);
    assert!(
        restarted.status.success(),
        "restart failed: {:?}",
        restarted.stderr
    );
    assert!(agent_exists(&state_root, "runtime-agent"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn installed_tui_creates_and_manages_an_existing_tree_project() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let worktree = directory.path().join("existing-worktree");
    std::fs::create_dir(&worktree).expect("existing working tree");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    let human = hq_output(&state_root, &["human", "create"]);
    assert!(
        human.status.success(),
        "human create failed: {:?}",
        human.stderr
    );

    let name = "tui-project";
    let path = worktree.to_str().expect("UTF-8 test path");
    let created = run_in_pty(
        &state_root,
        true,
        PtyInteraction::CreateExistingProject { name, path },
    );
    assert!(
        created.status.success(),
        "TUI create failed: {:?}",
        created.bytes
    );
    assert_eq!(
        created.before, created.after,
        "TUI did not restore terminal modes"
    );
    let listing = hq_output(&state_root, &["--output", "json", "project", "list"]);
    assert!(
        listing.status.success() && String::from_utf8_lossy(&listing.stdout).contains(name),
        "project list did not contain TUI project: {:?} {:?}",
        listing.stdout,
        listing.stderr
    );

    let project_id = project_id(&state_root, name);
    let sent = hq_output(
        &state_root,
        &["project", "send", &project_id, "installed project input"],
    );
    assert!(
        sent.status.success(),
        "CLI input fixture failed: {:?}",
        sent.stderr
    );

    let added_resource = directory.path().join("added-resource");
    std::fs::create_dir(&added_resource).expect("added resource creates");
    let added = run_in_pty(
        &state_root,
        true,
        PtyInteraction::AddProjectResource {
            name,
            path: added_resource.to_str().expect("UTF-8 resource path"),
        },
    );
    assert!(
        added.status.success(),
        "TUI resource add failed: {:?}",
        added.bytes
    );
    assert_eq!(
        added.before, added.after,
        "TUI did not restore terminal modes"
    );
    let resources = hq_output(
        &state_root,
        &[
            "--output",
            "json",
            "project",
            "resource",
            "list",
            &project_id,
        ],
    );
    assert!(
        resources.status.success()
            && String::from_utf8_lossy(&resources.stdout)
                .contains(added_resource.to_str().expect("UTF-8 resource path")),
        "TUI resource was not visible through the CLI: {:?} {:?}",
        resources.stdout,
        resources.stderr
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn installed_guided_work_dispatches_initial_and_follow_up_in_open_conversation() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let worktree = directory.path().join("guided-worktree");
    let provider_bin = directory.path().join("provider-bin");
    std::fs::create_dir(&worktree).expect("guided working tree");
    std::fs::create_dir(&provider_bin).expect("provider bin directory");
    install_fake_codex(&provider_bin);
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let search_path = format!("{}:{inherited_path}", provider_bin.display());

    initialize_identity(&state_root);
    let _daemon = start_foreground_daemon(&state_root, &search_path, &provider_bin.join("codex"));
    let human = hq_output_with_search_path(&state_root, &["human", "create"], &search_path);
    assert!(human.status.success(), "human create failed: {human:?}");

    let name = "guided-project";
    let agent = "guided-agent";
    let content = "implement the guided change";
    let run = run_in_pty(
        &state_root,
        true,
        PtyInteraction::CreateGuidedProjectWork {
            name,
            path: worktree.to_str().expect("UTF-8 guided path"),
            agent,
            content,
            search_path: &search_path,
        },
    );
    assert!(run.status.success(), "guided TUI failed: {:?}", run.bytes);
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
    for forbidden in [
        "Start project work",
        "project_activation_thread_missing",
        "HQ could not make this change",
    ] {
        assert!(
            !run.bytes
                .windows(forbidden.len())
                .any(|window| window == forbidden.as_bytes()),
            "guided TUI rendered forbidden text {forbidden:?}: {:?}",
            run.bytes
        );
    }
    let project = project_json(&state_root, name);
    assert_eq!(project["assignment"]["runnable"], true);
    assert_eq!(project["inputs"].as_array().map(Vec::len), Some(1));
    assert_eq!(project["dispatches"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        project["inputs"][0]["message_id"],
        project["dispatches"][0]["message_id"]
    );

    let follow_up = "continue with the installed follow-up";
    let reply = run_in_pty(
        &state_root,
        true,
        PtyInteraction::ReplyToProjectConversation {
            name,
            initial: content,
            content: follow_up,
        },
    );
    assert!(
        reply.status.success(),
        "guided reply failed: {:?}",
        reply.bytes
    );
    for phrase in ["Agent is working", "finished-turn-2"] {
        assert!(
            reply
                .bytes
                .windows(phrase.len())
                .any(|window| window == phrase.as_bytes()),
            "guided reply did not render {phrase:?}: {:?}",
            reply.bytes
        );
    }
    let project = project_json(&state_root, name);
    assert_eq!(project["inputs"].as_array().map(Vec::len), Some(2));
    assert_eq!(project["dispatches"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        project["inputs"][1]["message_id"],
        project["dispatches"][1]["message_id"]
    );
    assert!(mailbox_contains(&state_root, follow_up));
    assert!(mailbox_contains(&state_root, "finished-turn-2"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn installed_tui_project_lifecycle_matches_the_cli() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let worktree = directory.path().join("lifecycle-worktree");
    std::fs::create_dir(&worktree).expect("existing working tree");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    assert!(
        hq_output(&state_root, &["human", "create"])
            .status
            .success()
    );
    let name = "tui-lifecycle";
    let created = hq_output(
        &state_root,
        &[
            "project",
            "create",
            name,
            "--path",
            worktree.to_str().expect("UTF-8 path"),
        ],
    );
    assert!(
        created.status.success(),
        "project create failed: {created:?}"
    );

    for (interaction, lifecycle, archived) in [
        (PtyInteraction::CloseProject { name }, "closed", false),
        (PtyInteraction::OpenProject { name }, "open", false),
        (
            PtyInteraction::SetProjectArchived {
                name,
                archived: true,
            },
            "closed",
            true,
        ),
        (
            PtyInteraction::SetProjectArchived {
                name,
                archived: false,
            },
            "closed",
            false,
        ),
    ] {
        let run = run_in_pty(&state_root, true, interaction);
        assert!(
            run.status.success(),
            "TUI lifecycle failed: {:?}",
            run.bytes
        );
        assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
        let project = project_json(&state_root, name);
        assert_eq!(project["lifecycle"], lifecycle);
        assert_eq!(project["archived"], archived);
    }
}

#[test]
fn installed_tui_provisions_a_recoverable_git_worktree_project() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let repository = directory.path().join("repository");
    let destination = directory.path().join("tui-worktree");
    std::fs::create_dir(&repository).expect("repository creates");
    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(arguments)
            .output()
            .expect("Git runs");
        assert!(output.status.success(), "Git failed: {output:?}");
        String::from_utf8(output.stdout)
            .expect("Git output is UTF-8")
            .trim()
            .to_owned()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "hq@example.invalid"]);
    git(&["config", "user.name", "HQ Test"]);
    std::fs::write(repository.join("tracked.txt"), "initial\n").expect("tracked file writes");
    git(&["add", "tracked.txt"]);
    git(&["commit", "-qm", "initial"]);
    let base = git(&["rev-parse", "HEAD"]);

    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);
    let human = hq_output(&state_root, &["human", "create"]);
    assert!(
        human.status.success(),
        "human create failed: {:?}",
        human.stderr
    );
    let run = run_in_pty(
        &state_root,
        true,
        PtyInteraction::CreateWorktreeProject {
            name: "tui-worktree-project",
            source: repository.to_str().expect("UTF-8 source"),
            destination: destination.to_str().expect("UTF-8 destination"),
            branch: "feature/tui",
            base: &base,
        },
    );
    assert!(run.status.success(), "TUI worktree failed: {:?}", run.bytes);
    assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
    assert!(destination.join("tracked.txt").is_file());
    let branch = Command::new("git")
        .arg("-C")
        .arg(&destination)
        .args(["branch", "--show-current"])
        .output()
        .expect("Git worktree inspection runs");
    assert!(branch.status.success());
    assert_eq!(
        String::from_utf8_lossy(&branch.stdout).trim(),
        "feature/tui"
    );
}

#[test]
fn explicit_tui_without_terminal_fails_without_escape_sequences() {
    let _scenario = serial_scenario();
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--state-root")
        .arg(state_root)
        .arg("tui")
        .stdin(Stdio::null())
        .output()
        .expect("nonterminal TUI invocation runs");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tui.terminal_required"));
    assert!(!output.stderr.contains(&0x1b));
}

struct PtyRun {
    status: ExitStatus,
    bytes: Vec<u8>,
    before: Termios,
    after: Termios,
}

#[derive(Clone, Copy, Debug)]
enum PtyInteraction<'content> {
    QuitOnStart,
    QuitAfterSetup,
    OpenNewLauncher,
    CompleteFreshSetupAndReconnect,
    SubmitSelfNote(&'content str),
    SubmitPastedSelfNote {
        content: &'content str,
        marker: &'content str,
    },
    NavigateInboxConversation(&'content str),
    ReplyToProjectConversation {
        name: &'content str,
        initial: &'content str,
        content: &'content str,
    },
    CreateAgent(&'content str),
    StartRejectedSession,
    CreateExistingProject {
        name: &'content str,
        path: &'content str,
    },
    CreateGuidedProjectWork {
        name: &'content str,
        path: &'content str,
        agent: &'content str,
        content: &'content str,
        search_path: &'content str,
    },
    AddProjectResource {
        name: &'content str,
        path: &'content str,
    },
    CloseProject {
        name: &'content str,
    },
    OpenProject {
        name: &'content str,
    },
    SetProjectArchived {
        name: &'content str,
        archived: bool,
    },
    CreateWorktreeProject {
        name: &'content str,
        source: &'content str,
        destination: &'content str,
        branch: &'content str,
        base: &'content str,
    },
}

#[allow(clippy::too_many_lines)]
fn run_in_pty(state_root: &Path, explicit: bool, interaction: PtyInteraction<'_>) -> PtyRun {
    // Ratatui's differential output is not a screen transcript, so durable mutation completion
    // must synchronize against authoritative state instead of a possibly split rendered phrase.
    let agent_target = match interaction {
        PtyInteraction::CreateAgent(name) => Some(name),
        _ => None,
    };
    let lifecycle_target = match interaction {
        PtyInteraction::CloseProject { name } => Some((name, "closed", false)),
        PtyInteraction::OpenProject { name } => Some((name, "open", false)),
        PtyInteraction::SetProjectArchived { name, archived } => Some((name, "closed", archived)),
        _ => None,
    }
    .map(|(name, lifecycle, archived)| (project_id(state_root, name), lifecycle, archived));
    let dimensions = Winsize {
        ws_row: 30,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pair = openpty(Some(&dimensions), None).expect("pseudoterminal opens");
    let before = tcgetattr(&pair.slave).expect("initial terminal modes");
    let stdin = stdio_clone(&pair.slave);
    let stdout = stdio_clone(&pair.slave);
    let stderr = stdio_clone(&pair.slave);
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command
        .arg("--state-root")
        .arg(state_root)
        .env("TERM", "xterm-256color")
        .env_remove("NO_COLOR")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    if matches!(interaction, PtyInteraction::StartRejectedSession) {
        command.env("PATH", "/nonexistent");
    }
    if let PtyInteraction::CreateGuidedProjectWork { search_path, .. } = interaction {
        command.env("PATH", search_path);
    }
    if explicit {
        command.arg("tui");
    }
    let mut child = command.spawn().expect("TUI process starts");
    let mut master = File::from(pair.master);
    fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).expect("master is nonblocking");

    let mut last_output_at = Instant::now();
    let mut bytes = Vec::new();
    let mut initial_key_sent = false;
    let mut content_sent = false;
    let mut completion_offset = None;
    let mut managed_action_sent = false;
    let mut managed_provider_sent = false;
    let mut resource_commit_sent = false;
    let mut exit_sent = false;
    let mut next_state_probe_at = Instant::now();
    let status = loop {
        let previous_output_length = bytes.len();
        let mut buffer = [0_u8; 8192];
        match master.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) if error.raw_os_error() == Some(5) => {}
            Err(error) => panic!("pseudoterminal read failed: {error}"),
        }
        if bytes.len() > previous_output_length {
            last_output_at = Instant::now();
        }
        let alternate_screen_entered = bytes
            .windows(ENTER_ALTERNATE_SCREEN.len())
            .any(|window| window == ENTER_ALTERNATE_SCREEN);
        if matches!(interaction, PtyInteraction::CompleteFreshSetupAndReconnect)
            && !initial_key_sent
            && bytes
                .windows(b"No human account is selected".len())
                .any(|window| window == b"No human account is selected")
        {
            let setup = hq_output(state_root, &["human", "create"]);
            assert!(setup.status.success(), "account setup failed: {setup:?}");
            master.write_all(b"\x1b[15~").expect("F5 key writes");
            master.flush().expect("F5 key flushes");
            initial_key_sent = true;
            completion_offset = Some(bytes.len());
        }
        let connected = bytes
            .windows(b"Connected".len())
            .any(|window| window == b"Connected");
        let interaction_ready = match interaction {
            PtyInteraction::QuitOnStart => true,
            PtyInteraction::QuitAfterSetup => bytes
                .windows(b"No human account is selected".len())
                .any(|window| window == b"No human account is selected"),
            PtyInteraction::CompleteFreshSetupAndReconnect => false,
            _ => connected,
        };
        if !initial_key_sent
            && alternate_screen_entered
            && interaction_ready
            && !matches!(interaction, PtyInteraction::CompleteFreshSetupAndReconnect)
        {
            let keys: Vec<&[u8]> = match interaction {
                PtyInteraction::QuitOnStart | PtyInteraction::QuitAfterSetup => vec![b"q"],
                PtyInteraction::OpenNewLauncher => vec![b"n"],
                PtyInteraction::CompleteFreshSetupAndReconnect => unreachable!(),
                PtyInteraction::SubmitSelfNote(_) | PtyInteraction::SubmitPastedSelfNote { .. } => {
                    vec![b"N"]
                }
                PtyInteraction::NavigateInboxConversation(_)
                | PtyInteraction::ReplyToProjectConversation { .. } => Vec::new(),
                PtyInteraction::CreateAgent(_) => vec![b"l", b"l", b"l", b"c"],
                PtyInteraction::StartRejectedSession => vec![b"l", b"l", b"l", b"\t"],
                PtyInteraction::CreateExistingProject { .. } => {
                    vec![b"l", b"l", b"l", b"l", b"c", b"\r"]
                }
                PtyInteraction::CreateGuidedProjectWork { .. } => {
                    vec![b"n", b"\r", b"\r", b"\r"]
                }
                PtyInteraction::AddProjectResource { .. }
                | PtyInteraction::CloseProject { .. }
                | PtyInteraction::OpenProject { .. }
                | PtyInteraction::SetProjectArchived { .. } => {
                    vec![b"l", b"l", b"l", b"l", b"\t"]
                }
                PtyInteraction::CreateWorktreeProject { .. } => {
                    vec![b"l", b"l", b"l", b"l", b"w"]
                }
            };
            for key in keys {
                master.write_all(key).expect("initial TUI key writes");
                master.flush().expect("initial TUI key flushes");
                std::thread::sleep(Duration::from_millis(100));
            }
            initial_key_sent = true;
            exit_sent = matches!(
                interaction,
                PtyInteraction::QuitOnStart | PtyInteraction::QuitAfterSetup
            );
        }
        if matches!(interaction, PtyInteraction::CompleteFreshSetupAndReconnect)
            && initial_key_sent
            && !content_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Get started with HQ".len())
                    .any(|window| window == b"Get started with HQ")
            })
        {
            let restarted = hq_output(state_root, &["daemon", "restart"]);
            assert!(
                restarted.status.success(),
                "daemon restart failed: {restarted:?}"
            );
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CompleteFreshSetupAndReconnect)
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Connected".len())
                    .any(|window| window == b"Connected")
            })
        {
            master.write_all(b"n").expect("New launcher key writes");
            master.flush().expect("New launcher key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CompleteFreshSetupAndReconnect)
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Work with an agent on a project".len())
                    .any(|window| window == b"Work with an agent on a project")
            })
        {
            master.write_all(b"\x1bOP").expect("F1 key writes");
            master.flush().expect("F1 key flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CompleteFreshSetupAndReconnect)
            && managed_provider_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Help for New".len())
                    .any(|window| window == b"Help for New")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(interaction, PtyInteraction::OpenNewLauncher)
            && initial_key_sent
            && !exit_sent
            && bytes.windows(b"New".len()).any(|window| window == b"New")
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if let PtyInteraction::SubmitSelfNote(content) = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"0/16384 bytes".len())
                .any(|window| window == b"0/16384 bytes")
        {
            master
                .write_all(format!("{content}\r").as_bytes())
                .expect("self-note text writes");
            master.flush().expect("self-note text flushes");
            content_sent = true;
        }
        if let PtyInteraction::SubmitPastedSelfNote { content, .. } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"0/16384 bytes".len())
                .any(|window| window == b"0/16384 bytes")
        {
            master
                .write_all(format!("\x1b[200~{content}\x1b[201~\r").as_bytes())
                .expect("bracketed self-note paste writes");
            master.flush().expect("bracketed self-note paste flushes");
            content_sent = true;
        }
        if let PtyInteraction::NavigateInboxConversation(content) = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(content.len())
                .any(|window| window == content.as_bytes())
            && bytes.windows(b"You".len()).any(|window| window == b"You")
        {
            master
                .write_all(b"\t\r")
                .expect("Inbox conversation entry keys write");
            master.flush().expect("Inbox conversation entry keys flush");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::NavigateInboxConversation(_))
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows("h/← Inbox".len())
                    .any(|window| window == "h/← Inbox".as_bytes())
            })
        {
            master.write_all(b"h").expect("Inbox back key writes");
            master.flush().expect("Inbox back key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::NavigateInboxConversation(_))
            && managed_action_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Enter open".len())
                    .any(|window| window == b"Enter open")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if let PtyInteraction::ReplyToProjectConversation { name, initial, .. } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(name.len())
                .any(|window| window == name.as_bytes())
            && bytes
                .windows(initial.len())
                .any(|window| window == initial.as_bytes())
        {
            master
                .write_all(b"\t\r")
                .expect("project conversation entry keys write");
            master
                .flush()
                .expect("project conversation entry keys flush");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(
            interaction,
            PtyInteraction::ReplyToProjectConversation { .. }
        ) && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows("h/← Inbox".len())
                    .any(|window| window == "h/← Inbox".as_bytes())
            })
        {
            master.write_all(b"r").expect("project reply key writes");
            master.flush().expect("project reply key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::ReplyToProjectConversation { content, .. } = interaction
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Continue project conversation".len())
                    .any(|window| window == b"Continue project conversation")
                    && bytes[offset..]
                        .windows(b"0/16384 bytes".len())
                        .any(|window| window == b"0/16384 bytes")
            })
        {
            master
                .write_all(format!("{content}\r").as_bytes())
                .expect("project follow-up writes");
            master.flush().expect("project follow-up flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(
            interaction,
            PtyInteraction::ReplyToProjectConversation { .. }
        ) && managed_provider_sent
            && !resource_commit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Agent is working".len())
                    .any(|window| window == b"Agent is working")
            })
        {
            resource_commit_sent = true;
        }
        if let PtyInteraction::ReplyToProjectConversation { name, .. } = interaction
            && resource_commit_sent
            && !exit_sent
            && Instant::now() >= next_state_probe_at
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"finished-turn-2".len())
                    .any(|window| window == b"finished-turn-2")
            })
        {
            next_state_probe_at = Instant::now() + AUTHORITATIVE_STATE_PROBE_INTERVAL;
            if project_has_dispatch_count(state_root, name, 2) {
                master.write_all(&[0x03]).expect("Ctrl-C writes");
                master.flush().expect("Ctrl-C flushes");
                exit_sent = true;
            }
        }
        if let PtyInteraction::CreateAgent(name) = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"lowercase".len())
                .any(|window| window == b"lowercase")
        {
            master
                .write_all(format!("{name}\r").as_bytes())
                .expect("agent name writes");
            master.flush().expect("agent name flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::StartRejectedSession)
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"runtime-agent".len())
                .any(|window| window == b"runtime-agent")
        {
            master.write_all(b"\r").expect("agent details key writes");
            master.flush().expect("agent details key flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::CreateExistingProject { name, path } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"Name:".len())
                .any(|window| window == b"Name:")
        {
            master
                .write_all(format!("\t{name}\x1b[Z{path}\r").as_bytes())
                .expect("project form writes");
            master.flush().expect("project form flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::CreateGuidedProjectWork { name, path, .. } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"Name:".len())
                .any(|window| window == b"Name:")
        {
            master
                .write_all(format!("\t{name}\x1b[Z{path}\r").as_bytes())
                .expect("guided project form writes");
            master.flush().expect("guided project form flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CreateGuidedProjectWork { .. })
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Create an agent".len())
                    .any(|window| window == b"Create an agent")
            })
        {
            master.write_all(b"\r").expect("guided agent choice writes");
            master.flush().expect("guided agent choice flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::CreateGuidedProjectWork { agent, .. } = interaction
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Name:".len())
                    .any(|window| window == b"Name:")
            })
        {
            master
                .write_all(format!("{agent}\r").as_bytes())
                .expect("guided agent name writes");
            master.flush().expect("guided agent name flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::CreateGuidedProjectWork { content, .. } = interaction
            && managed_provider_sent
            && !resource_commit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"New project conversation".len())
                    .any(|window| window == b"New project conversation")
                    && bytes[offset..]
                        .windows(b"0/16384 bytes".len())
                        .any(|window| window == b"0/16384 bytes")
            })
        {
            master
                .write_all(format!("{content}\r").as_bytes())
                .expect("guided instruction writes");
            master.flush().expect("guided instruction flushes");
            resource_commit_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::CreateWorktreeProject {
            name,
            source,
            destination,
            branch,
            base,
        } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"Source:".len())
                .any(|window| window == b"Source:")
        {
            master
                .write_all(
                    format!("{name}\t\t{source}\t{destination}\t{branch}\t{base}\r").as_bytes(),
                )
                .expect("worktree project form writes");
            master.flush().expect("worktree project form flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::AddProjectResource { name, .. } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(name.len())
                .any(|window| window == name.as_bytes())
        {
            master.write_all(b"l").expect("project summary key writes");
            master.flush().expect("project details key flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        let lifecycle_name = match interaction {
            PtyInteraction::CloseProject { name }
            | PtyInteraction::OpenProject { name }
            | PtyInteraction::SetProjectArchived { name, .. } => Some(name),
            _ => None,
        };
        if let Some(name) = lifecycle_name
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(name.len())
                .any(|window| window == name.as_bytes())
        {
            master.write_all(b"l").expect("project summary key writes");
            master.flush().expect("project details key flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CloseProject { .. })
            && content_sent
            && !managed_action_sent
        {
            master
                .write_all(b"jjj\rjj\r")
                .expect("close assessment action writes");
            master.flush().expect("close assessment key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CloseProject { .. })
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Esc close".len())
                    .any(|window| window == b"Esc close")
            })
        {
            master.write_all(b"\r").expect("close assessment accepts");
            master.flush().expect("close assessment flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CloseProject { .. })
            && managed_provider_sent
            && !resource_commit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Tab/Shift-Tab".len())
                    .any(|window| window == b"Tab/Shift-Tab")
            })
        {
            master
                .write_all(b"cf\r")
                .expect("confirmed forced close writes");
            master.flush().expect("confirmed forced close flushes");
            resource_commit_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::OpenProject { .. })
            && content_sent
            && !managed_action_sent
        {
            master
                .write_all(b"jjj\r")
                .expect("open project management writes");
            master.flush().expect("reopen key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::OpenProject { .. })
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| bytes.len() > offset)
        {
            master
                .write_all(b"j\r")
                .expect("reopen project action writes");
            master.flush().expect("reopen recovery flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::SetProjectArchived { .. })
            && content_sent
            && !managed_action_sent
        {
            let action_keys = if let PtyInteraction::SetProjectArchived { archived, .. } =
                interaction
                && archived
            {
                b"jjj\rjjj\r".as_slice()
            } else {
                b"jjj\rj\r".as_slice()
            };
            master
                .write_all(action_keys)
                .expect("archive action writes");
            master.flush().expect("archive choice key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::SetProjectArchived { archived, .. } = interaction
            && managed_action_sent
            && !managed_provider_sent
        {
            let title = if archived {
                b"Archiving".as_slice()
            } else {
                b"Unarchiving".as_slice()
            };
            if completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(title.len())
                    .any(|window| window == title)
            }) {
                master
                    .write_all(b"\r")
                    .expect("archive confirmation writes");
                master.flush().expect("archive confirmation flushes");
                managed_provider_sent = true;
                completion_offset = Some(bytes.len());
            }
        }
        if matches!(interaction, PtyInteraction::AddProjectResource { .. })
            && content_sent
            && !managed_action_sent
        {
            master
                .write_all(b"jjj\r\r\r")
                .expect("resource add action writes");
            master.flush().expect("resource add key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::AddProjectResource { path, .. } = interaction
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Path:".len())
                    .any(|window| window == b"Path:")
            })
        {
            master
                .write_all(format!("{path}\r").as_bytes())
                .expect("resource path writes");
            master.flush().expect("resource path flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::AddProjectResource { .. })
            && managed_provider_sent
            && !resource_commit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Resolved".len())
                    .any(|window| window == b"Resolved")
            })
        {
            master.write_all(b"\r").expect("resource commit writes");
            master.flush().expect("resource commit flushes");
            resource_commit_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::StartRejectedSession)
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Saved conversations".len())
                    .any(|window| window == b"Saved conversations")
            })
        {
            master.write_all(b"s").expect("managed start key writes");
            master.flush().expect("managed start key flushes");
            managed_action_sent = true;
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::SubmitSelfNote(content) = interaction
            && content_sent
            && !exit_sent
            && Instant::now() >= next_state_probe_at
        {
            next_state_probe_at = Instant::now() + AUTHORITATIVE_STATE_PROBE_INTERVAL;
            if mailbox_contains(state_root, content) {
                master.write_all(&[0x03]).expect("Ctrl-C writes");
                master.flush().expect("Ctrl-C flushes");
                exit_sent = true;
            }
        }
        if let PtyInteraction::SubmitPastedSelfNote { marker, .. } = interaction
            && content_sent
            && !exit_sent
            && Instant::now() >= next_state_probe_at
        {
            next_state_probe_at = Instant::now() + AUTHORITATIVE_STATE_PROBE_INTERVAL;
            if mailbox_contains(state_root, marker) {
                master.write_all(&[0x03]).expect("Ctrl-C writes");
                master.flush().expect("Ctrl-C flushes");
                exit_sent = true;
            }
        }
        let lifecycle_action_sent = resource_commit_sent
            && !matches!(interaction, PtyInteraction::CreateGuidedProjectWork { .. })
            || managed_action_sent && matches!(interaction, PtyInteraction::OpenProject { .. })
            || managed_provider_sent
                && matches!(interaction, PtyInteraction::SetProjectArchived { .. });
        let agent_action_sent = content_sent && agent_target.is_some();
        if (lifecycle_action_sent || agent_action_sent)
            && !exit_sent
            && Instant::now() >= next_state_probe_at
        {
            next_state_probe_at = Instant::now() + AUTHORITATIVE_STATE_PROBE_INTERVAL;
            let lifecycle_reached =
                lifecycle_target
                    .as_ref()
                    .is_some_and(|(project_id, lifecycle, archived)| {
                        project_has_state(state_root, project_id, lifecycle, *archived)
                    });
            let agent_reached = agent_target.is_some_and(|name| agent_exists(state_root, name));
            if lifecycle_reached || agent_reached {
                master.write_all(&[0x03]).expect("Ctrl-C writes");
                master.flush().expect("Ctrl-C flushes");
                exit_sent = true;
            }
        }
        if let PtyInteraction::CreateGuidedProjectWork { name, .. } = interaction
            && resource_commit_sent
            && !exit_sent
            && Instant::now() >= next_state_probe_at
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Project agent".len())
                    .any(|window| window == b"Project agent")
            })
        {
            next_state_probe_at = Instant::now() + AUTHORITATIVE_STATE_PROBE_INTERVAL;
            if project_is_runnable_with_one_dispatch(state_root, name) {
                master.write_all(&[0x03]).expect("Ctrl-C writes");
                master.flush().expect("Ctrl-C flushes");
                exit_sent = true;
            }
        }
        if matches!(interaction, PtyInteraction::CreateExistingProject { .. })
            && content_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Project created".len())
                    .any(|window| window == b"Project created")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(interaction, PtyInteraction::CreateWorktreeProject { .. })
            && content_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Project created".len())
                    .any(|window| window == b"Project created")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(interaction, PtyInteraction::AddProjectResource { .. })
            && resource_commit_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Project updated".len())
                    .any(|window| window == b"Project updated")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(interaction, PtyInteraction::StartRejectedSession)
            && managed_provider_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Technical".len())
                    .any(|window| window == b"Technical")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if let Some(status) = child.try_wait().expect("TUI process status") {
            break status;
        }
        if Instant::now().duration_since(last_output_at) >= PROCESS_INACTIVITY_WATCHDOG {
            let _ = child.kill();
            let _ = child.wait();
            let provider_calls = match interaction {
                PtyInteraction::CreateGuidedProjectWork { search_path, .. } => search_path
                    .split(':')
                    .next()
                    .map(|directory| {
                        std::fs::read_to_string(Path::new(directory).join("calls.log"))
                    })
                    .transpose()
                    .unwrap_or_else(|error| Some(format!("unreadable: {error}"))),
                _ => None,
            };
            let guided_project = match interaction {
                PtyInteraction::CreateGuidedProjectWork { .. } => Some(
                    String::from_utf8_lossy(
                        &hq_output(state_root, &["--output", "json", "project", "list"]).stdout,
                    )
                    .into_owned(),
                ),
                _ => None,
            };
            panic!(
                "TUI process timed out for {interaction:?} (initial={initial_key_sent}, content={content_sent}, action={managed_action_sent}, provider={managed_provider_sent}, resource={resource_commit_sent}, exit={exit_sent}); provider calls: {provider_calls:?}; project: {guided_project:?}; output: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    loop {
        let mut buffer = [0_u8; 8192];
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(5) =>
            {
                break;
            }
            Err(error) => panic!("final pseudoterminal read failed: {error}"),
        }
    }
    let after = tcgetattr(&pair.slave).expect("restored terminal modes");
    PtyRun {
        status,
        bytes,
        before,
        after,
    }
}

fn serial_scenario() -> MutexGuard<'static, ()> {
    PSEUDOTERMINAL_SCENARIO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mailbox_contains(state_root: &Path, content: &str) -> bool {
    let output = hq_output(state_root, &["--output", "json", "list", "--all"]);
    output.status.success() && String::from_utf8_lossy(&output.stdout).contains(content)
}

fn agent_exists(state_root: &Path, name: &str) -> bool {
    let output = hq_output(state_root, &["--output", "json", "agent", "show", name]);
    output.status.success() && String::from_utf8_lossy(&output.stdout).contains(name)
}

fn project_id(state_root: &Path, name: &str) -> String {
    let output = hq_output(state_root, &["--output", "json", "project", "list"]);
    assert!(output.status.success(), "project list failed: {output:?}");
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("project list is JSON");
    value["data"]["projects"]
        .as_array()
        .expect("project array")
        .iter()
        .find(|project| project["name"] == name)
        .and_then(|project| project["project_id"].as_str())
        .expect("project identity")
        .to_owned()
}

fn project_json(state_root: &Path, name: &str) -> serde_json::Value {
    let id = project_id(state_root, name);
    let output = hq_output(state_root, &["--output", "json", "project", "show", &id]);
    assert!(output.status.success(), "project show failed: {output:?}");
    serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("project show is JSON")["data"]
        ["projects"][0]
        .clone()
}

fn project_has_state(state_root: &Path, project_id: &str, lifecycle: &str, archived: bool) -> bool {
    let output = hq_output(
        state_root,
        &["--output", "json", "project", "show", project_id],
    );
    if !output.status.success() {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return false;
    };
    value["data"]["projects"]
        .as_array()
        .and_then(|projects| projects.first())
        .is_some_and(|project| project["lifecycle"] == lifecycle && project["archived"] == archived)
}

fn project_is_runnable_with_one_dispatch(state_root: &Path, name: &str) -> bool {
    let project = project_json(state_root, name);
    project["assignment"]["runnable"] == true
        && project["inputs"].as_array().map(Vec::len) == Some(1)
        && project["dispatches"].as_array().map(Vec::len) == Some(1)
        && project["inputs"][0]["message_id"] == project["dispatches"][0]["message_id"]
}

fn project_has_dispatch_count(state_root: &Path, name: &str, expected: usize) -> bool {
    let project = project_json(state_root, name);
    project["inputs"].as_array().map(Vec::len) == Some(expected)
        && project["dispatches"].as_array().map(Vec::len) == Some(expected)
}

fn hq_output(state_root: &Path, arguments: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command.arg("--state-root").arg(state_root);
    for argument in arguments {
        command.arg(argument);
    }
    command
        .stdin(Stdio::null())
        .output()
        .expect("installed HQ command runs")
}

fn hq_output_with_search_path(
    state_root: &Path,
    arguments: &[&str],
    search_path: &str,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hq"));
    command
        .arg("--state-root")
        .arg(state_root)
        .env("PATH", search_path);
    for argument in arguments {
        command.arg(argument);
    }
    command
        .stdin(Stdio::null())
        .output()
        .expect("installed HQ command runs with provider search path")
}

fn install_fake_codex(directory: &Path) {
    let executable = directory.join("codex");
    std::fs::write(
        &executable,
        r#"#!/usr/bin/python3
import json
import os
import sys
import time

thread_id = "hq-test-thread"
turn_number = 0
for line in sys.stdin:
    with open(os.path.join(os.path.dirname(__file__), "calls.log"), "a") as log:
        log.write(line)
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if request_id is None:
        continue
    if method == "initialize":
        result = {}
    elif method == "thread/start":
        result = {"thread": {"id": thread_id, "turns": []}}
    elif method == "thread/resume":
        thread_id = message.get("params", {}).get("threadId", thread_id)
        result = {"thread": {"id": thread_id, "turns": []}}
    elif method == "turn/start":
        turn_number += 1
        turn_id = f"hq-test-turn-{turn_number}"
        turn = {"id": turn_id, "status": "inProgress", "items": []}
        print(json.dumps({"method": "turn/started", "params": {"threadId": thread_id, "turn": turn}}), flush=True)
        print(json.dumps({"id": request_id, "result": {"turn": turn}}), flush=True)
        time.sleep(0.25)
        item = {"type": "agentMessage", "id": f"answer-{turn_number}", "text": f"finished-turn-{turn_number}", "phase": "final_answer"}
        print(json.dumps({"method": "item/completed", "params": {"threadId": thread_id, "turnId": turn_id, "item": item}}), flush=True)
        completed = {"id": turn_id, "status": "completed", "items": [item]}
        print(json.dumps({"method": "turn/completed", "params": {"threadId": thread_id, "turn": completed}}), flush=True)
        continue
    elif method == "thread/read":
        result = {"thread": {"id": thread_id, "turns": []}}
    elif method in ("turn/interrupt", "turn/steer"):
        result = {"turnId": "hq-test-turn"}
    else:
        continue
    print(json.dumps({"id": request_id, "result": result}), flush=True)
"#,
    )
    .expect("fake Codex executable writes");
    let mut permissions = std::fs::metadata(&executable)
        .expect("fake Codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(executable, permissions).expect("fake Codex is executable");
}

fn stdio_clone(descriptor: &OwnedFd) -> Stdio {
    Stdio::from(File::from(
        descriptor.try_clone().expect("terminal descriptor clones"),
    ))
}

fn initialize_identity(state_root: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--output")
        .arg("json")
        .arg("--state-root")
        .arg(state_root)
        .arg("identity")
        .arg("init")
        .stdin(Stdio::null())
        .output()
        .expect("identity initialization runs");
    assert!(
        output.status.success(),
        "identity initialization failed: {:?}",
        output.stderr
    );
}

struct DaemonStopGuard<'state>(&'state Path);

impl Drop for DaemonStopGuard<'_> {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_hq"))
            .arg("--state-root")
            .arg(self.0)
            .arg("daemon")
            .arg("stop")
            .stdin(Stdio::null())
            .output();
    }
}

struct ForegroundDaemonGuard<'state> {
    state_root: &'state Path,
    child: Option<std::process::Child>,
}

impl Drop for ForegroundDaemonGuard<'_> {
    fn drop(&mut self) {
        let _ = hq_output(self.state_root, &["daemon", "stop"]);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn start_foreground_daemon<'state>(
    state_root: &'state Path,
    search_path: &str,
    provider_executable: &Path,
) -> ForegroundDaemonGuard<'state> {
    let child = Command::new(env!("CARGO_BIN_EXE_hq"))
        .arg("--state-root")
        .arg(state_root)
        .args(["daemon", "run"])
        .env("PATH", search_path)
        .env("CODEX_BIN", provider_executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("foreground daemon starts");
    let guard = ForegroundDaemonGuard {
        state_root,
        child: Some(child),
    };
    let started = Instant::now();
    loop {
        let status = hq_output(state_root, &["daemon", "status"]);
        if status.status.success() && String::from_utf8_lossy(&status.stdout).contains("ready") {
            return guard;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "foreground daemon did not become ready: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
