//! Installed terminal routing and pseudoterminal restoration contracts.

#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::expect_used, clippy::panic)]

mod support;

use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    os::fd::OwnedFd,
    path::Path,
    process::{Command, ExitStatus, Stdio},
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
// This is a deadlock watchdog for installed process tests, not a product latency budget.
const PROCESS_COMPLETION_WATCHDOG: Duration = Duration::from_secs(30);

#[test]
fn explicit_and_bare_tui_render_and_restore_the_pseudoterminal() {
    let directory = TestDirectory::new();
    let state_root = directory.path().join("state");
    initialize_identity(&state_root);
    let _daemon = DaemonStopGuard(&state_root);

    for explicit in [true, false] {
        let run = run_in_pty(&state_root, explicit, PtyInteraction::QuitOnStart);
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
                .windows(LEAVE_ALTERNATE_SCREEN.len())
                .any(|window| window == LEAVE_ALTERNATE_SCREEN),
            "TUI did not leave the alternate screen"
        );
        assert_eq!(run.before, run.after, "TUI did not restore terminal modes");
    }
}

#[test]
fn installed_tui_self_note_matches_cli_and_survives_restart() {
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
fn installed_tui_agent_create_matches_cli_and_survives_restart() {
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
fn installed_tui_starts_explicit_provider_and_renders_typed_rejection() {
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
            .windows(b"Rejected:".len())
            .any(|window| window == b"Rejected:"),
        "TUI did not render the typed rejected outcome: {:?}",
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
fn installed_tui_creates_an_existing_tree_project_and_sends_input() {
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

    let content = "installed project input";
    let sent = run_in_pty(
        &state_root,
        true,
        PtyInteraction::SendProjectInput { name, content },
    );
    assert!(sent.status.success(), "TUI input failed: {:?}", sent.bytes);
    assert_eq!(
        sent.before, sent.after,
        "TUI did not restore terminal modes"
    );
    assert!(
        sent.bytes
            .windows(b"Project operation outcome".len())
            .any(|window| window == b"Project operation outcome"),
        "TUI did not render typed input completion: {:?}",
        sent.bytes
    );

    let dispatched = run_in_pty(
        &state_root,
        true,
        PtyInteraction::DispatchProjectInput { name },
    );
    assert!(
        dispatched.status.success(),
        "TUI dispatch failed: {:?}",
        dispatched.bytes
    );
    assert!(
        dispatched
            .bytes
            .windows(b"Rejected:".len())
            .any(|window| window == b"Rejected:"),
        "unassigned dispatch did not retain typed rejection: {:?}",
        dispatched.bytes
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
    let project_id = project_id(&state_root, name);
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
fn installed_tui_project_lifecycle_matches_the_cli() {
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

#[derive(Clone, Copy)]
enum PtyInteraction<'content> {
    QuitOnStart,
    SubmitSelfNote(&'content str),
    CreateAgent(&'content str),
    StartRejectedSession,
    CreateExistingProject {
        name: &'content str,
        path: &'content str,
    },
    SendProjectInput {
        name: &'content str,
        content: &'content str,
    },
    DispatchProjectInput {
        name: &'content str,
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
    let dimensions = Winsize {
        ws_row: 30,
        ws_col: 100,
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
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    if explicit {
        command.arg("tui");
    }
    let mut child = command.spawn().expect("TUI process starts");
    let mut master = File::from(pair.master);
    fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).expect("master is nonblocking");

    let deadline = Instant::now() + PROCESS_COMPLETION_WATCHDOG;
    let mut bytes = Vec::new();
    let mut initial_key_sent = false;
    let mut content_sent = false;
    let mut completion_offset = None;
    let mut managed_action_sent = false;
    let mut managed_provider_sent = false;
    let mut resource_commit_sent = false;
    let mut exit_sent = false;
    let status = loop {
        let mut buffer = [0_u8; 8192];
        match master.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) if error.raw_os_error() == Some(5) => {}
            Err(error) => panic!("pseudoterminal read failed: {error}"),
        }
        let alternate_screen_entered = bytes
            .windows(ENTER_ALTERNATE_SCREEN.len())
            .any(|window| window == ENTER_ALTERNATE_SCREEN);
        if !initial_key_sent && alternate_screen_entered {
            let key = match interaction {
                PtyInteraction::QuitOnStart => b"q".as_slice(),
                PtyInteraction::SubmitSelfNote(_) => b"n".as_slice(),
                PtyInteraction::CreateAgent(_) => b"lllc".as_slice(),
                PtyInteraction::StartRejectedSession => b"lll".as_slice(),
                PtyInteraction::CreateExistingProject { .. } => b"llllc".as_slice(),
                PtyInteraction::SendProjectInput { .. }
                | PtyInteraction::DispatchProjectInput { .. }
                | PtyInteraction::AddProjectResource { .. }
                | PtyInteraction::CloseProject { .. }
                | PtyInteraction::OpenProject { .. }
                | PtyInteraction::SetProjectArchived { .. } => b"llll".as_slice(),
                PtyInteraction::CreateWorktreeProject { .. } => b"llllw".as_slice(),
            };
            master.write_all(key).expect("initial TUI key writes");
            master.flush().expect("initial TUI key flushes");
            initial_key_sent = true;
            exit_sent = matches!(interaction, PtyInteraction::QuitOnStart);
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
        if let PtyInteraction::CreateAgent(name) = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(b"Permanent".len())
                .any(|window| window == b"Permanent")
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
                .write_all(format!("{name}\x1b[B\x1b[B{path}\r").as_bytes())
                .expect("project form writes");
            master.flush().expect("project form flushes");
            content_sent = true;
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
                    format!(
                        "{name}\x1b[B\x1b[B{source}\x1b[B{destination}\x1b[B{branch}\x1b[B{base}\r"
                    )
                    .as_bytes(),
                )
                .expect("worktree project form writes");
            master.flush().expect("worktree project form flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::SendProjectInput { name, .. } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(name.len())
                .any(|window| window == name.as_bytes())
        {
            master.write_all(b"\r").expect("project details key writes");
            master.flush().expect("project details key flushes");
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
            master.write_all(b"\r").expect("project details key writes");
            master.flush().expect("project details key flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::DispatchProjectInput { name } = interaction
            && initial_key_sent
            && !content_sent
            && bytes
                .windows(name.len())
                .any(|window| window == name.as_bytes())
        {
            master.write_all(b"\r").expect("project details key writes");
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
            master.write_all(b"\r").expect("project details key writes");
            master.flush().expect("project details key flushes");
            content_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CloseProject { .. })
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Unassigned".len())
                    .any(|window| window == b"Unassigned")
            })
        {
            master.write_all(b"c").expect("close assessment key writes");
            master.flush().expect("close assessment key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::CloseProject { .. })
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"operation outcome".len())
                    .any(|window| window == b"operation outcome")
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
                    .windows(b"Fresh release assessment".len())
                    .any(|window| window == b"Fresh release assessment")
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
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Unassigned".len())
                    .any(|window| window == b"Unassigned")
            })
        {
            master.write_all(b"o").expect("reopen key writes");
            master.flush().expect("reopen key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::SetProjectArchived { .. })
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Unassigned".len())
                    .any(|window| window == b"Unassigned")
            })
        {
            master.write_all(b"z").expect("archive choice key writes");
            master.flush().expect("archive choice key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::SetProjectArchived { archived, .. } = interaction
            && managed_action_sent
            && !managed_provider_sent
        {
            let title = if archived {
                b"Archiving closes the project".as_slice()
            } else {
                b"unarchive ".as_slice()
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
        if matches!(interaction, PtyInteraction::DispatchProjectInput { .. })
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Unassigned".len())
                    .any(|window| window == b"Unassigned")
            })
        {
            master.write_all(b"d").expect("project dispatch key writes");
            master.flush().expect("project dispatch key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::AddProjectResource { .. })
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"a add".len())
                    .any(|window| window == b"a add")
            })
        {
            master.write_all(b"a").expect("resource add key writes");
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
                    .windows(b"Canonical:".len())
                    .any(|window| window == b"Canonical:")
            })
        {
            master.write_all(b"\r").expect("resource commit writes");
            master.flush().expect("resource commit flushes");
            resource_commit_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::SendProjectInput { content, .. } = interaction
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"send".len())
                    .any(|window| window == b"send")
            })
        {
            master.write_all(b"n").expect("project input key writes");
            master.flush().expect("project input key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
            let _ = content;
        }
        if let PtyInteraction::SendProjectInput { content, .. } = interaction
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Input:".len())
                    .any(|window| window == b"Input:")
            })
        {
            master
                .write_all(format!("{content}\r").as_bytes())
                .expect("project input writes");
            master.flush().expect("project input flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::StartRejectedSession)
            && content_sent
            && !managed_action_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Durable sessions".len())
                    .any(|window| window == b"Durable sessions")
            })
        {
            master.write_all(b"s").expect("managed start key writes");
            master.flush().expect("managed start key flushes");
            managed_action_sent = true;
            completion_offset = Some(bytes.len());
        }
        if matches!(interaction, PtyInteraction::StartRejectedSession)
            && managed_action_sent
            && !managed_provider_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Provider namespace".len())
                    .any(|window| window == b"Provider namespace")
            })
        {
            master
                .write_all(b"unregistered\r")
                .expect("managed provider writes");
            master.flush().expect("managed provider flushes");
            managed_provider_sent = true;
            completion_offset = Some(bytes.len());
        }
        if let PtyInteraction::SubmitSelfNote(content) = interaction
            && content_sent
            && !exit_sent
            && bytes
                .windows(b"open messages".len())
                .any(|window| window == b"open messages")
        {
            let _ = content;
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(
            interaction,
            PtyInteraction::CloseProject { .. }
                | PtyInteraction::OpenProject { .. }
                | PtyInteraction::SetProjectArchived { .. }
        ) && (resource_commit_sent
            || managed_action_sent && matches!(interaction, PtyInteraction::OpenProject { .. })
            || managed_provider_sent
                && matches!(interaction, PtyInteraction::SetProjectArchived { .. }))
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Completed".len())
                    .any(|window| window == b"Completed")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(interaction, PtyInteraction::CreateExistingProject { .. })
            && content_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Completed".len())
                    .any(|window| window == b"Completed")
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
                    .windows(b"Completed".len())
                    .any(|window| window == b"Completed")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(interaction, PtyInteraction::SendProjectInput { .. })
            && managed_provider_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Project operation outcome".len())
                    .any(|window| window == b"Project operation outcome")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if matches!(interaction, PtyInteraction::DispatchProjectInput { .. })
            && managed_action_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"Rejected:".len())
                    .any(|window| window == b"Rejected:")
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
                    .windows(b"at head".len())
                    .any(|window| window == b"at head")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if let PtyInteraction::CreateAgent(_) = interaction
            && content_sent
            && !exit_sent
            && completion_offset.is_some_and(|offset| {
                bytes[offset..]
                    .windows(b"revision ".len())
                    .any(|window| window == b"revision ")
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
                    .windows(b"Rejected:".len())
                    .any(|window| window == b"Rejected:")
            })
        {
            master.write_all(&[0x03]).expect("Ctrl-C writes");
            master.flush().expect("Ctrl-C flushes");
            exit_sent = true;
        }
        if let Some(status) = child.try_wait().expect("TUI process status") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("TUI process timed out; bytes: {bytes:?}");
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
