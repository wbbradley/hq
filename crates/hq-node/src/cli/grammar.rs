//! State-free installed command grammar and mapping.

use std::{ffi::OsString, path::PathBuf, time::Duration};

use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command, error::ErrorKind, value_parser};
use hq_domain::{
    AccountId, AgentId, BoundedText, ContentText, EncryptionPublicKey, InstallationId, MailboxId,
    MessageId, ProjectId, ProviderId, ProviderSessionId, RESOURCE_LOCATOR_MAX_BYTES, RelayHints,
    ResourceLocator, ResourceScheme, ShortText, SigningPublicKey, ThreadId,
};
use hq_local_api::protocol::v1::{RelayAccessDto, RelayAuthenticationDto};

use super::{
    AgentGuidanceTopic, AgentMailboxSelection, AgentMessageCommand, CliCommand, CliError,
    CliInvocation, CliOutputFormat, ConfigurationCommand, DaemonCommand, HarnessCommand,
    HumanCommand, HumanMessageCommand, HumanMessageFilters, IdentityCommand, MailboxCommand,
    NamedAgentCommand, NamedAgentSelector, PeerCommand, ProjectCliCommand,
    ProjectResourceCliCommand, RelayCommand, ThemeSelection, WorktreeCliRequest, parsed_state,
};

const ID: &str = "64 lowercase hexadecimal characters";

pub(super) fn parse(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<CliInvocation, CliError> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if arguments.is_empty() {
        return Ok(CliInvocation {
            output: CliOutputFormat::Human,
            command: CliCommand::Help { topic: Vec::new() },
        });
    }
    let mut argv = Vec::with_capacity(arguments.len() + 1);
    argv.push(OsString::from("hq"));
    argv.extend(arguments.iter().cloned());
    let matches = match command().try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            return Ok(CliInvocation {
                output: output_hint(&arguments),
                command: CliCommand::Help {
                    topic: help_flag_topic(&arguments),
                },
            });
        }
        Err(error) if error.kind() == ErrorKind::DisplayVersion => {
            return Ok(CliInvocation {
                output: output_hint(&arguments),
                command: CliCommand::Version,
            });
        }
        Err(_) => return Err(CliError::Arguments),
    };
    map_invocation(&matches)
}

pub(super) fn help(topic: &[String]) -> Result<String, CliError> {
    let mut selected = command();
    selected.build();
    for segment in topic {
        selected = selected
            .find_subcommand(segment)
            .cloned()
            .ok_or(CliError::Arguments)?;
    }
    let mut rendered = Vec::new();
    selected
        .write_long_help(&mut rendered)
        .map_err(|_| CliError::Runtime)?;
    String::from_utf8(rendered).map_err(|_| CliError::Runtime)
}

fn command() -> Command {
    Command::new("hq")
        .about("HQ local client")
        .version(env!("CARGO_PKG_VERSION"))
        .disable_help_subcommand(true)
        .arg(
            Arg::new("output")
                .long("output")
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .global(true)
                .help("Select human text or hq-cli-output-v1 JSON records"),
        )
        .arg(
            Arg::new("state-root")
                .long("state-root")
                .value_name("ABSOLUTE_PATH")
                .value_parser(value_parser!(PathBuf))
                .global(true)
                .help("Select an installation state root"),
        )
        .subcommands(vec![
            help_command(),
            leaf("version", "Show build and protocol metadata"),
            Command::new("tui")
                .about("Open the interactive terminal interface")
                .long_about("Open the interactive terminal interface. Both stdin and stdout must be attached to a terminal."),
            agents_command(),
            agent_command(),
            harness_command(),
            project_command(),
            agent_message_command("ask"),
            agent_message_command("send"),
            agent_message_command("wait"),
            agent_message_command("poll"),
            Command::new("get")
                .about("Inspect one message without consuming it")
                .long_about("Inspect one projected or dependency-incomplete message without consuming it.")
                .arg(id_arg("message", "MESSAGE_ID")),
            human_list_command(),
            human_message_command("answer"),
            human_message_command("cancel"),
            human_message_command("archive"),
            human_message_command("restore"),
            Command::new("mailboxes")
                .about("Discover repository-aware session mailboxes")
                .long_about("List durable provider sessions joined with typed directory, repository, worktree, and branch context. Discovery never claims or merges mailboxes.")
                .arg(path_option("directory", "dir", "PATH")),
            identity_command(),
            config_command(),
            human_command(),
            peer_command(),
            mailbox_command(),
            relay_command(),
            daemon_command(),
        ])
}

fn leaf(name: &'static str, about: &'static str) -> Command {
    Command::new(name).about(about)
}

fn help_command() -> Command {
    Command::new("help")
        .about("Show generated help for a command path")
        .arg(Arg::new("topic").value_name("COMMAND").num_args(0..))
}

fn agents_command() -> Command {
    Command::new("agents")
        .about("Show concise installed guidance for agents")
        .arg(Arg::new("topic").value_parser([
            "messaging",
            "retry",
            "synchronization",
            "delivery",
            "causality",
            "administration",
        ]))
}

fn agent_command() -> Command {
    Command::new("agent")
        .about("Manage named agents and durable session metadata")
        .long_about("Manage named agents and durable session metadata. Names are permanent lowercase installation-local slugs. Provider/session options must be supplied together; ambiguous provider environments and stale or conflicted session metadata are rejected. Retirement is absorbing, requires --yes, and only --force may revoke HQ authority after failed or uncertain runtime cessation.")
        .subcommands([
            leaf("list", "List every named agent"),
            Command::new("show").about("Show one named agent").arg(agent_arg()),
            Command::new("create")
                .about("Create or adopt a named agent mailbox")
                .arg(name_arg())
                .arg(Arg::new("mailbox").long("mailbox").value_name("MAILBOX_ID")),
            leaf("current", "Resolve the current provider session binding"),
            Command::new("select")
                .about("Select an agent for one durable provider session")
                .arg(agent_arg())
                .args(mailbox_selection_args()),
            Command::new("rename")
                .about("Set or clear a provider-session display name")
                .arg(agent_arg())
                .arg(Arg::new("name").value_name("DISPLAY_NAME"))
                .arg(Arg::new("clear").long("clear").action(ArgAction::SetTrue).conflicts_with("name"))
                .group(ArgGroup::new("rename-choice").args(["name", "clear"]).required(true))
                .args(provider_session_args()),
            Command::new("retire")
                .about("Permanently retire one named agent")
                .arg(agent_arg())
                .arg(required_confirmation())
                .arg(force_arg()),
        ])
}

fn harness_command() -> Command {
    Command::new("harness")
        .about("Control managed provider sessions")
        .long_about("Control managed provider sessions. Start and resume resolve an absolute launch directory and copy the caller environment at the local API boundary. Resume requires an exact session and never falls back to a new session. Rejected and uncertain operations remain distinct for reconciliation.")
        .subcommands([
            harness_leaf("start").arg(path_option("directory", "dir", "PATH")),
            harness_leaf("resume")
                .arg(Arg::new("session").long("session").required(true).value_name("SESSION"))
                .arg(path_option("directory", "dir", "PATH")),
            harness_leaf("stop"),
        ])
}

fn harness_leaf(name: &'static str) -> Command {
    Command::new(name)
        .arg(
            Arg::new("agent")
                .long("agent")
                .required(true)
                .value_name("NAME|AGENT_ID"),
        )
        .arg(
            Arg::new("provider")
                .long("provider")
                .required(true)
                .value_name("PROVIDER"),
        )
}

fn project_command() -> Command {
    Command::new("project")
        .about("Inspect and control authoritative projects")
        .long_about("Inspect and control authoritative projects. Resources express project ownership without turning HQ into a worktree manager: resource commands never mutate or delete filesystem or Git state, and worktree provisioning never prunes, resets, removes, or compensates external Git state. Work sent while a project is closed or unassigned remains pending. Assignment requires an explicit new-session or exact existing-session choice; close and handoff require --yes, while --force is separate authority for blocked or uncertain release. Conflicted history remains explicit and HQ never chooses a historical winner.")
        .subcommands([
            leaf("list", "List authoritative projects"),
            Command::new("show").arg(id_arg("project", "PROJECT_ID")),
            project_resource_command(),
            Command::new("check")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(Arg::new("resource").value_name("RESOURCE_ID")),
            Command::new("send")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(Arg::new("message").value_name("MESSAGE")),
            Command::new("create")
                .arg(name_arg())
                .arg(path_option("path", "path", "ABSOLUTE_PATH").required(true))
                .arg(Arg::new("brief").long("brief").value_name("TEXT"))
                .arg(Arg::new("home").long("home").value_name("INSTALLATION_ID")),
            Command::new("worktree")
                .arg(name_arg())
                .arg(path_option("source", "source", "ABSOLUTE_PATH").required(true))
                .arg(path_option("destination", "destination", "ABSOLUTE_PATH").required(true))
                .arg(Arg::new("branch").long("branch").required(true).value_name("BRANCH"))
                .arg(Arg::new("create-branch").long("create-branch").value_name("BASE"))
                .arg(Arg::new("brief").long("brief").value_name("TEXT"))
                .arg(Arg::new("home").long("home").value_name("INSTALLATION_ID")),
            Command::new("open").arg(id_arg("project", "PROJECT_ID")),
            assignment_command("activate", false),
            Command::new("dispatch").arg(id_arg("project", "PROJECT_ID")),
            assignment_command("handoff", true),
            Command::new("close")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(required_confirmation())
                .arg(force_arg()),
            Command::new("archive").arg(id_arg("project", "PROJECT_ID")),
            Command::new("unarchive").arg(id_arg("project", "PROJECT_ID")),
        ])
}

fn project_resource_command() -> Command {
    Command::new("resource")
        .about("Inspect or change desired project resources")
        .subcommands([
            Command::new("list").arg(id_arg("project", "PROJECT_ID")),
            Command::new("show")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(id_arg("resource", "RESOURCE_ID")),
            Command::new("add")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(path_option("path", "path", "ABSOLUTE_PATH").required(true))
                .arg(
                    Arg::new("primary")
                        .long("primary")
                        .help("Make the added resource the project's launch default")
                        .action(ArgAction::SetTrue),
                ),
            Command::new("remove")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(id_arg("resource", "RESOURCE_ID"))
                .arg(force_arg()),
            Command::new("replace")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(id_arg("resource", "RESOURCE_ID"))
                .arg(path_option("path", "path", "ABSOLUTE_PATH").required(true)),
            Command::new("primary")
                .arg(id_arg("project", "PROJECT_ID"))
                .arg(id_arg("resource", "RESOURCE_ID")),
        ])
}

fn assignment_command(name: &'static str, handoff: bool) -> Command {
    let mut command = Command::new(name)
        .arg(id_arg("project", "PROJECT_ID"))
        .arg(
            Arg::new("agent")
                .long("agent")
                .required(true)
                .value_name("NAME|AGENT_ID"),
        )
        .arg(
            Arg::new("provider")
                .long("provider")
                .required(true)
                .value_name("PROVIDER"),
        )
        .arg(
            Arg::new("session")
                .long("session")
                .value_name("SESSION")
                .conflicts_with("new-session"),
        )
        .arg(
            Arg::new("new-session")
                .long("new-session")
                .action(ArgAction::SetTrue)
                .conflicts_with("session"),
        )
        .group(
            ArgGroup::new("session-choice")
                .args(["session", "new-session"])
                .required(true),
        )
        .arg(Arg::new("thread").long("thread").value_name("THREAD_ID"))
        .arg(path_option("directory", "dir", "ABSOLUTE_PATH"));
    if handoff {
        command = command.arg(required_confirmation()).arg(force_arg());
    }
    command
}

fn agent_message_command(name: &'static str) -> Command {
    let mut command = Command::new(name)
        .about("Use one agent mailbox")
        .args(mailbox_selection_args());
    if matches!(name, "ask" | "wait") {
        command = command
            .long_about("Wait using bounded local API attempts. The overall wait is intentionally unbounded unless --timeout is supplied; retries retain stable message IDs.")
            .arg(Arg::new("timeout").long("timeout").value_name("DURATION"))
            .arg(Arg::new("interval").long("interval").value_name("DURATION"));
    }
    match name {
        "ask" | "send" => command.arg(Arg::new("message").value_name("MESSAGE")),
        "wait" => command.arg(id_arg("message", "MESSAGE_ID")),
        _ => command,
    }
}

fn human_list_command() -> Command {
    Command::new("list")
        .about("Filter the human mailbox")
        .arg(Arg::new("sender").long("sender").value_name("MAILBOX_ID"))
        .arg(
            Arg::new("recipient")
                .long("recipient")
                .value_name("MAILBOX_ID"),
        )
        .arg(
            Arg::new("archived")
                .long("archived")
                .action(ArgAction::SetTrue)
                .conflicts_with("all"),
        )
        .arg(
            Arg::new("all")
                .long("all")
                .action(ArgAction::SetTrue)
                .conflicts_with("archived"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_name("N")
                .default_value("100"),
        )
}

fn human_message_command(name: &'static str) -> Command {
    let about = match name {
        "answer" => "Answer one question as the human",
        "cancel" => "Cancel one human-authored question",
        "archive" => "Archive one message",
        "restore" => "Restore one archived message",
        _ => "Manage one human mailbox message",
    };
    let command = Command::new(name)
        .about(about)
        .long_about("Message content may be supplied as one argument or bounded UTF-8 stdin where supported. Dependency-incomplete records are inert and cannot authorize answer, cancel, archive, or restore operations.")
        .arg(id_arg("message", "MESSAGE_ID"));
    if name == "answer" {
        command.arg(Arg::new("response").value_name("RESPONSE"))
    } else {
        command
    }
}

fn identity_command() -> Command {
    Command::new("identity")
        .about("Manage installation identity offline")
        .long_about("Manage installation identity offline. Backup passwords are read as one bounded UTF-8 line from stdin and are never accepted as an argument.")
        .subcommands([
            leaf("init", "Create identity without overwrite"),
            leaf("show", "Show safe public identity metadata"),
            Command::new("export").arg(path_arg("path", "ABSOLUTE_PATH")).arg(password_stdin_arg()),
            Command::new("import").arg(path_arg("path", "ABSOLUTE_PATH")).arg(password_stdin_arg()),
        ])
}

fn config_command() -> Command {
    Command::new("config")
        .about("Manage typed local defaults offline")
        .long_about("Manage typed local defaults under exclusive offline ownership.")
        .subcommands([
            leaf("get", "Show all local defaults"),
            leaf("themes", "List bundled and user-defined TUI themes"),
            Command::new("set").subcommands([
                Command::new("default-provider").arg(
                    Arg::new("provider")
                        .required(true)
                        .value_name("PROVIDER|none"),
                ),
                Command::new("relays").arg(
                    Arg::new("relays")
                        .required(true)
                        .num_args(1..)
                        .value_name("URL|none"),
                ),
                Command::new("theme")
                    .about("Select a startup TUI theme or restore automatic selection")
                    .arg(
                        Arg::new("theme")
                            .required(true)
                            .value_name("NAME|ABSOLUTE_PATH|none"),
                    ),
            ]),
        ])
}

fn human_command() -> Command {
    Command::new("human")
        .about("Manage the local human account")
        .long_about("Manage the local human account through application plans. Account selection and authority are resolved from one authoritative snapshot; HQ never guesses among candidates.")
        .subcommands([
            Command::new("create").arg(Arg::new("label").value_name("LABEL")),
            leaf("show", "Show authoritative account and selection state"),
            Command::new("select").arg(id_arg("account", "ACCOUNT_ID")),
            Command::new("invite")
                .arg(id_arg("installation", "INSTALLATION_ID"))
                .arg(id_arg("signing", "SIGNING_KEY"))
                .arg(path_arg("destination", "ABSOLUTE_PATH"))
                .args(pairing_args()),
            Command::new("join").arg(path_arg("source", "ABSOLUTE_PATH")),
            leaf("devices", "Show complete selected-account device history"),
            Command::new("revoke").arg(id_arg("installation", "INSTALLATION_ID")),
        ])
}

fn peer_command() -> Command {
    Command::new("peer")
        .about("Manage directional peer routes")
        .long_about("Manage directional peer routes. Routes are metadata only and never grant mailbox authority; distrust revokes active local capabilities before blocking the route.")
        .subcommands([
            Command::new("add")
                .arg(id_arg("installation", "INSTALLATION_ID"))
                .arg(id_arg("signing", "SIGNING_KEY"))
                .arg(id_arg("encryption", "ENCRYPTION_KEY"))
                .args(pairing_args()),
            leaf("list", "List complete route history"),
            Command::new("distrust").arg(id_arg("installation", "INSTALLATION_ID")),
        ])
}

fn mailbox_command() -> Command {
    Command::new("mailbox")
        .about("Manage directional mailbox capabilities")
        .long_about("Manage directional mailbox capabilities. Each mutation requires an exact locally owned mailbox and a uniquely routable peer.")
        .subcommands([
            leaf("list", "List mailbox capabilities"),
            Command::new("grant")
                .arg(id_arg("mailbox", "MAILBOX_ID"))
                .arg(id_arg("peer", "PEER_INSTALLATION_ID")),
            Command::new("revoke")
                .arg(id_arg("mailbox", "MAILBOX_ID"))
                .arg(id_arg("peer", "PEER_INSTALLATION_ID")),
        ])
}

fn relay_command() -> Command {
    Command::new("relay")
        .about("Manage relay policy, synchronization, and health")
        .long_about("Manage relay policy, synchronization, and bounded health. Removing a relay disables policy without erasing delivery history; repair explicitly reverifies immutable evidence and replaces only rebuildable indexes.")
        .subcommands([
            Command::new("add")
                .arg(Arg::new("endpoint").required(true).value_name("URL"))
                .arg(
                    Arg::new("access")
                        .long("access")
                        .value_parser(["read", "write", "read-write"])
                        .default_value("read-write"),
                )
                .arg(
                    Arg::new("auth")
                        .long("auth")
                        .value_parser(["disabled", "on-challenge", "required"])
                        .default_value("on-challenge"),
                ),
            leaf("list", "List relay policies"),
            Command::new("remove").arg(Arg::new("endpoint").required(true).value_name("URL")),
            Command::new("sync").arg(Arg::new("endpoint").value_name("URL")),
            leaf("status", "Show bounded relay health"),
            leaf(
                "repair",
                "Reverify the corpus and replace rebuildable indexes",
            ),
        ])
}

fn daemon_command() -> Command {
    Command::new("daemon")
        .about("Manage the local node lifecycle")
        .subcommands([
            leaf("run", "Own the node in the foreground"),
            leaf("status", "Probe without starting a node"),
            leaf("readiness", "Return a ready node, starting when absent"),
            leaf("stop", "Converge the node to absence"),
            leaf("restart", "Converge on a fresh ready generation"),
        ])
}

fn id_arg(id: &'static str, value_name: &'static str) -> Arg {
    Arg::new(id).required(true).value_name(value_name).help(ID)
}

fn name_arg() -> Arg {
    Arg::new("name")
        .required(true)
        .value_name("NAME")
        .help("Human-readable name")
}

fn agent_arg() -> Arg {
    Arg::new("agent")
        .required(true)
        .value_name("NAME|AGENT_ID")
        .help("Permanent agent name or exact agent identity")
}

fn path_arg(id: &'static str, value_name: &'static str) -> Arg {
    Arg::new(id)
        .required(true)
        .value_name(value_name)
        .value_parser(value_parser!(PathBuf))
        .help("Absolute operating-system path")
}

fn path_option(id: &'static str, long: &'static str, value_name: &'static str) -> Arg {
    Arg::new(id)
        .long(long)
        .value_name(value_name)
        .value_parser(value_parser!(PathBuf))
        .help("Operating-system path selected for this operation")
}

fn provider_session_args() -> [Arg; 2] {
    [
        Arg::new("provider")
            .long("provider")
            .value_name("PROVIDER")
            .help("Provider namespace; requires --session")
            .requires("session"),
        Arg::new("session")
            .long("session")
            .value_name("SESSION")
            .help("Exact provider session; requires --provider")
            .requires("provider"),
    ]
}

fn mailbox_selection_args() -> [Arg; 3] {
    [
        Arg::new("provider")
            .long("provider")
            .value_name("PROVIDER")
            .help("Provider namespace; requires --session")
            .requires("session"),
        Arg::new("session")
            .long("session")
            .value_name("SESSION")
            .help("Exact provider session; requires --provider")
            .requires("provider"),
        path_option("directory", "dir", "PATH"),
    ]
}

fn pairing_args() -> [Arg; 2] {
    [
        Arg::new("label").long("label").value_name("LABEL"),
        Arg::new("relay")
            .long("relay")
            .value_name("URL")
            .help("Repeatable relay hint")
            .action(ArgAction::Append),
    ]
}

fn required_confirmation() -> Arg {
    Arg::new("yes")
        .long("yes")
        .required(true)
        .help("Confirm the requested state change")
        .action(ArgAction::SetTrue)
}

fn force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .help("Authorize the documented blocked or uncertain recovery path")
        .action(ArgAction::SetTrue)
}

fn password_stdin_arg() -> Arg {
    Arg::new("password-stdin")
        .long("password-stdin")
        .required(true)
        .help("Read one bounded backup password from stdin")
        .action(ArgAction::SetTrue)
}

fn map_invocation(matches: &ArgMatches) -> Result<CliInvocation, CliError> {
    let output = match text(matches, "output")? {
        "human" => CliOutputFormat::Human,
        "json" => CliOutputFormat::Json,
        _ => return Err(CliError::Arguments),
    };
    let state_root = matches.get_one::<PathBuf>("state-root");
    let state = || parsed_state(state_root);
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    let command = match name {
        "help" => CliCommand::Help {
            topic: args
                .get_many::<String>("topic")
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
        },
        "version" => CliCommand::Version,
        "tui" if output == CliOutputFormat::Human => CliCommand::Tui { state: state()? },
        "agents" => CliCommand::AgentGuidance {
            topic: AgentGuidanceTopic::parse(args.get_one::<String>("topic").map(String::as_str))
                .ok_or(CliError::Arguments)?,
        },
        "agent" => CliCommand::NamedAgent {
            action: map_agent(args)?,
            state: state()?,
        },
        "harness" => CliCommand::Harness {
            action: map_harness(args)?,
            state: state()?,
        },
        "project" => CliCommand::Project {
            action: map_project(args)?,
            state: state()?,
        },
        "ask" | "send" | "wait" | "poll" => CliCommand::AgentMessage {
            action: map_agent_message(name, args)?,
            state: state()?,
        },
        "get" => CliCommand::GetMessage {
            message_id: MessageId::from_bytes(hex(args, "message")?),
            state: state()?,
        },
        "mailboxes" => CliCommand::DiscoverMailboxes {
            directory: args.get_one::<PathBuf>("directory").cloned(),
            state: state()?,
        },
        "list" | "answer" | "cancel" | "archive" | "restore" => CliCommand::HumanMessage {
            action: map_human_message(name, args)?,
            state: state()?,
        },
        "identity" => CliCommand::Identity {
            action: map_identity(args)?,
            state: state()?,
        },
        "config" => CliCommand::Configuration {
            action: map_config(args)?,
            state: state()?,
        },
        "human" => CliCommand::Human {
            action: map_human(args)?,
            state: state()?,
        },
        "peer" => CliCommand::Peer {
            action: map_peer(args)?,
            state: state()?,
        },
        "mailbox" => CliCommand::Mailbox {
            action: map_mailbox(args)?,
            state: state()?,
        },
        "relay" => CliCommand::Relay {
            action: map_relay(args)?,
            state: state()?,
        },
        "daemon" => CliCommand::Daemon {
            action: map_daemon(args)?,
            state: state()?,
        },
        _ => return Err(CliError::Arguments),
    };
    if state_root.is_some()
        && matches!(
            command,
            CliCommand::Help { .. } | CliCommand::Version | CliCommand::AgentGuidance { .. }
        )
    {
        return Err(CliError::Arguments);
    }
    Ok(CliInvocation { output, command })
}

fn map_agent(matches: &ArgMatches) -> Result<NamedAgentCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "list" => NamedAgentCommand::List,
        "show" => NamedAgentCommand::Show {
            agent: agent(args, "agent")?,
        },
        "create" => NamedAgentCommand::Create {
            name: agent_name(args, "name")?,
            mailbox_id: args
                .get_one::<String>("mailbox")
                .map(|_| hex(args, "mailbox").map(MailboxId::from_bytes))
                .transpose()?,
        },
        "current" => NamedAgentCommand::Current,
        "select" => NamedAgentCommand::Select {
            agent: agent(args, "agent")?,
            mailbox: mailbox_selection(args)?,
        },
        "rename" => NamedAgentCommand::Rename {
            agent: agent(args, "agent")?,
            provider: optional_provider(args)?,
            session: optional_session(args)?,
            display_name: args
                .get_one::<String>("name")
                .map(|value| ShortText::new(value.clone()).map_err(|_| CliError::Arguments))
                .transpose()?,
        },
        "retire" => NamedAgentCommand::Retire {
            agent: agent(args, "agent")?,
            force: args.get_flag("force"),
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_harness(matches: &ArgMatches) -> Result<HarnessCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    let agent = option_agent(args, "agent")?;
    let provider = provider(args, "provider")?;
    let directory = args
        .try_get_one::<PathBuf>("directory")
        .ok()
        .flatten()
        .cloned();
    Ok(match name {
        "start" => HarnessCommand::Start {
            agent,
            provider,
            directory,
        },
        "resume" => HarnessCommand::Resume {
            agent,
            provider,
            session: session(args, "session")?,
            directory,
        },
        "stop" => HarnessCommand::Stop { agent, provider },
        _ => return Err(CliError::Arguments),
    })
}

fn map_project(matches: &ArgMatches) -> Result<ProjectCliCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "list" => ProjectCliCommand::List,
        "show" => ProjectCliCommand::Show(ProjectId::from_bytes(hex(args, "project")?)),
        "resource" => ProjectCliCommand::Resource(map_project_resource(args)?),
        "check" => ProjectCliCommand::Check {
            project_id: ProjectId::from_bytes(hex(args, "project")?),
            resource_id: args
                .get_one::<String>("resource")
                .map(|_| hex(args, "resource").map(hq_domain::ResourceId::from_bytes))
                .transpose()?,
        },
        "send" => ProjectCliCommand::Send {
            project_id: ProjectId::from_bytes(hex(args, "project")?),
            body: optional_content(args, "message")?,
        },
        "create" => {
            let path = normalized_path(args, "path")?;
            ProjectCliCommand::Create {
                name: short_text(args, "name")?,
                brief: optional_content(args, "brief")?,
                path,
                home: optional_installation(args, "home")?,
            }
        }
        "worktree" => ProjectCliCommand::Worktree(WorktreeCliRequest {
            name: short_text(args, "name")?,
            brief: optional_content(args, "brief")?,
            source: normalized_path(args, "source")?,
            destination: normalized_path(args, "destination")?,
            branch: short_text(args, "branch")?,
            base: optional_short_text(args, "create-branch")?,
            home: optional_installation(args, "home")?,
        }),
        "open" => ProjectCliCommand::Open(ProjectId::from_bytes(hex(args, "project")?)),
        "activate" => map_assignment(args, false)?,
        "dispatch" => ProjectCliCommand::Dispatch(ProjectId::from_bytes(hex(args, "project")?)),
        "handoff" => map_assignment(args, true)?,
        "close" => ProjectCliCommand::Close {
            project_id: ProjectId::from_bytes(hex(args, "project")?),
            force: args.get_flag("force"),
        },
        "archive" => ProjectCliCommand::Archive(ProjectId::from_bytes(hex(args, "project")?)),
        "unarchive" => ProjectCliCommand::Unarchive(ProjectId::from_bytes(hex(args, "project")?)),
        _ => return Err(CliError::Arguments),
    })
}

fn map_project_resource(matches: &ArgMatches) -> Result<ProjectResourceCliCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    let project_id = ProjectId::from_bytes(hex(args, "project")?);
    Ok(match name {
        "list" => ProjectResourceCliCommand::List { project_id },
        "show" => ProjectResourceCliCommand::Show {
            project_id,
            resource_id: hq_domain::ResourceId::from_bytes(hex(args, "resource")?),
        },
        "add" => ProjectResourceCliCommand::Add {
            project_id,
            path: normalized_path(args, "path")?,
            make_primary: args.get_flag("primary"),
        },
        "remove" => ProjectResourceCliCommand::Remove {
            project_id,
            resource_id: hq_domain::ResourceId::from_bytes(hex(args, "resource")?),
            force: args.get_flag("force"),
        },
        "replace" => ProjectResourceCliCommand::Replace {
            project_id,
            resource_id: hq_domain::ResourceId::from_bytes(hex(args, "resource")?),
            path: normalized_path(args, "path")?,
        },
        "primary" => ProjectResourceCliCommand::Primary {
            project_id,
            resource_id: hq_domain::ResourceId::from_bytes(hex(args, "resource")?),
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_assignment(args: &ArgMatches, handoff: bool) -> Result<ProjectCliCommand, CliError> {
    let project_id = ProjectId::from_bytes(hex(args, "project")?);
    let agent = option_agent(args, "agent")?;
    let provider = provider(args, "provider")?;
    let resume_session = optional_session(args)?;
    let thread = args
        .get_one::<String>("thread")
        .map(|_| hex(args, "thread").map(ThreadId::from_bytes))
        .transpose()?;
    if resume_session.is_some() && thread.is_none() {
        return Err(CliError::Arguments);
    }
    let directory = args
        .get_one::<PathBuf>("directory")
        .map(|_| normalized_path(args, "directory"))
        .transpose()?;
    if handoff {
        Ok(ProjectCliCommand::Handoff {
            project_id,
            agent,
            provider,
            resume_session,
            thread_id: thread.ok_or(CliError::Arguments)?,
            directory,
            force: args.get_flag("force"),
        })
    } else {
        Ok(ProjectCliCommand::Activate {
            project_id,
            agent,
            provider,
            resume_session,
            resume_thread: thread,
            directory,
        })
    }
}

fn map_agent_message(name: &str, args: &ArgMatches) -> Result<AgentMessageCommand, CliError> {
    let mailbox = mailbox_selection(args)?;
    let timeout = args
        .try_get_one::<String>("timeout")
        .ok()
        .flatten()
        .map(|value| duration(value))
        .transpose()?;
    let interval = args
        .try_get_one::<String>("interval")
        .ok()
        .flatten()
        .map_or(Ok(Duration::from_millis(250)), |value| duration(value))?;
    Ok(match name {
        "ask" => AgentMessageCommand::Ask {
            mailbox,
            body: optional_content(args, "message")?,
            timeout,
            interval,
        },
        "send" => AgentMessageCommand::Send {
            mailbox,
            body: optional_content(args, "message")?,
        },
        "wait" => AgentMessageCommand::Wait {
            mailbox,
            message_id: MessageId::from_bytes(hex(args, "message")?),
            timeout,
            interval,
        },
        "poll" => AgentMessageCommand::Poll { mailbox },
        _ => return Err(CliError::Arguments),
    })
}

fn map_human_message(name: &str, args: &ArgMatches) -> Result<HumanMessageCommand, CliError> {
    Ok(match name {
        "list" => {
            let limit = text(args, "limit")?
                .parse::<u16>()
                .map_err(|_| CliError::Arguments)?;
            if !(1..=200).contains(&limit) {
                return Err(CliError::Arguments);
            }
            HumanMessageCommand::List(HumanMessageFilters {
                sender: optional_mailbox(args, "sender")?,
                recipient: optional_mailbox(args, "recipient")?,
                archived: args.get_flag("archived"),
                all: args.get_flag("all"),
                limit,
            })
        }
        "answer" => HumanMessageCommand::Answer {
            message_id: MessageId::from_bytes(hex(args, "message")?),
            body: optional_content(args, "response")?,
        },
        "cancel" => HumanMessageCommand::Cancel {
            message_id: MessageId::from_bytes(hex(args, "message")?),
        },
        "archive" => HumanMessageCommand::Archive {
            message_id: MessageId::from_bytes(hex(args, "message")?),
        },
        "restore" => HumanMessageCommand::Restore {
            message_id: MessageId::from_bytes(hex(args, "message")?),
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_identity(matches: &ArgMatches) -> Result<IdentityCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "init" => IdentityCommand::Init,
        "show" => IdentityCommand::Show,
        "export" => IdentityCommand::Export {
            destination: absolute(path(args, "path")?)?,
        },
        "import" => IdentityCommand::Import {
            source: absolute(path(args, "path")?)?,
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_config(matches: &ArgMatches) -> Result<ConfigurationCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    if name == "get" {
        return Ok(ConfigurationCommand::Get);
    }
    if name == "themes" {
        return Ok(ConfigurationCommand::Themes);
    }
    let (name, args) = args.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "default-provider" => ConfigurationCommand::SetDefaultProvider {
            provider: match text(args, "provider")? {
                "none" => None,
                value => Some(ProviderId::new(value).map_err(|_| CliError::Arguments)?),
            },
        },
        "relays" => {
            let values = args
                .get_many::<String>("relays")
                .ok_or(CliError::Arguments)?
                .collect::<Vec<_>>();
            let relays = if values.len() == 1 && values[0] == "none" {
                Vec::new()
            } else if values.iter().any(|value| *value == "none") {
                return Err(CliError::Arguments);
            } else {
                values
                    .into_iter()
                    .map(|value| relay_text(value))
                    .collect::<Result<Vec<_>, _>>()?
            };
            ConfigurationCommand::SetRelays { relays }
        }
        "theme" => ConfigurationCommand::SetTheme {
            theme: match text(args, "theme")? {
                "none" => None,
                value => {
                    Some(ThemeSelection::new(value.to_owned()).map_err(|_| CliError::Arguments)?)
                }
            },
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_human(matches: &ArgMatches) -> Result<HumanCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "create" => HumanCommand::Create {
            label: optional_short_text(args, "label")?,
        },
        "show" => HumanCommand::Show,
        "select" => HumanCommand::Select {
            account_id: AccountId::from_bytes(hex(args, "account")?),
        },
        "invite" => {
            let (label, relay_hints) = pairing(args)?;
            HumanCommand::Invite {
                installation_id: InstallationId::from_bytes(hex(args, "installation")?),
                signing_key: SigningPublicKey::from_bytes(hex(args, "signing")?),
                destination: absolute(path(args, "destination")?)?,
                label,
                relay_hints,
            }
        }
        "join" => HumanCommand::Join {
            source: absolute(path(args, "source")?)?,
        },
        "devices" => HumanCommand::Devices,
        "revoke" => HumanCommand::Revoke {
            installation_id: InstallationId::from_bytes(hex(args, "installation")?),
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_peer(matches: &ArgMatches) -> Result<PeerCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "add" => {
            let (label, relay_hints) = pairing(args)?;
            PeerCommand::Add {
                installation_id: InstallationId::from_bytes(hex(args, "installation")?),
                signing_key: SigningPublicKey::from_bytes(hex(args, "signing")?),
                encryption_key: EncryptionPublicKey::from_bytes(hex(args, "encryption")?),
                label,
                relay_hints,
            }
        }
        "list" => PeerCommand::List,
        "distrust" => PeerCommand::Distrust {
            installation_id: InstallationId::from_bytes(hex(args, "installation")?),
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_mailbox(matches: &ArgMatches) -> Result<MailboxCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "list" => MailboxCommand::List,
        "grant" => MailboxCommand::Grant {
            mailbox_id: MailboxId::from_bytes(hex(args, "mailbox")?),
            peer_id: InstallationId::from_bytes(hex(args, "peer")?),
        },
        "revoke" => MailboxCommand::Revoke {
            mailbox_id: MailboxId::from_bytes(hex(args, "mailbox")?),
            peer_id: InstallationId::from_bytes(hex(args, "peer")?),
        },
        _ => return Err(CliError::Arguments),
    })
}

fn map_relay(matches: &ArgMatches) -> Result<RelayCommand, CliError> {
    let (name, args) = matches.subcommand().ok_or(CliError::Arguments)?;
    Ok(match name {
        "add" => RelayCommand::Add {
            endpoint: relay(args, "endpoint")?,
            access: match text(args, "access")? {
                "read" => RelayAccessDto::Read,
                "write" => RelayAccessDto::Write,
                "read-write" => RelayAccessDto::ReadWrite,
                _ => return Err(CliError::Arguments),
            },
            authentication: match text(args, "auth")? {
                "disabled" => RelayAuthenticationDto::Disabled,
                "on-challenge" => RelayAuthenticationDto::OnChallenge,
                "required" => RelayAuthenticationDto::Required,
                _ => return Err(CliError::Arguments),
            },
        },
        "list" => RelayCommand::List,
        "remove" => RelayCommand::Remove {
            endpoint: relay(args, "endpoint")?,
        },
        "sync" => RelayCommand::Sync {
            endpoint: args
                .get_one::<String>("endpoint")
                .map(|value| relay_text(value))
                .transpose()?,
        },
        "status" => RelayCommand::Status,
        "repair" => RelayCommand::Repair,
        _ => return Err(CliError::Arguments),
    })
}

fn map_daemon(matches: &ArgMatches) -> Result<DaemonCommand, CliError> {
    Ok(
        match matches.subcommand_name().ok_or(CliError::Arguments)? {
            "run" => DaemonCommand::Run,
            "status" => DaemonCommand::Status,
            "readiness" => DaemonCommand::Readiness,
            "stop" => DaemonCommand::Stop,
            "restart" => DaemonCommand::Restart,
            _ => return Err(CliError::Arguments),
        },
    )
}

fn text<'a>(matches: &'a ArgMatches, id: &str) -> Result<&'a str, CliError> {
    matches
        .get_one::<String>(id)
        .map(String::as_str)
        .ok_or(CliError::Arguments)
}
fn path(matches: &ArgMatches, id: &str) -> Result<PathBuf, CliError> {
    matches
        .get_one::<PathBuf>(id)
        .cloned()
        .ok_or(CliError::Arguments)
}
fn hex(matches: &ArgMatches, id: &str) -> Result<[u8; 32], CliError> {
    decode_hex32(text(matches, id)?)
}
fn short_text(matches: &ArgMatches, id: &str) -> Result<ShortText, CliError> {
    ShortText::new(text(matches, id)?.to_owned()).map_err(|_| CliError::Arguments)
}
fn optional_short_text(matches: &ArgMatches, id: &str) -> Result<Option<ShortText>, CliError> {
    matches
        .get_one::<String>(id)
        .map(|value| ShortText::new(value.clone()).map_err(|_| CliError::Arguments))
        .transpose()
}
fn optional_content(matches: &ArgMatches, id: &str) -> Result<Option<ContentText>, CliError> {
    matches
        .get_one::<String>(id)
        .map(|value| content(value))
        .transpose()
}
fn agent(matches: &ArgMatches, id: &str) -> Result<NamedAgentSelector, CliError> {
    agent_selector(text(matches, id)?)
}
fn option_agent(matches: &ArgMatches, id: &str) -> Result<NamedAgentSelector, CliError> {
    agent(matches, id)
}
fn agent_name(matches: &ArgMatches, id: &str) -> Result<ShortText, CliError> {
    validated_agent_name(text(matches, id)?)
}
fn provider(matches: &ArgMatches, id: &str) -> Result<ProviderId, CliError> {
    ProviderId::new(text(matches, id)?).map_err(|_| CliError::Arguments)
}
fn session(matches: &ArgMatches, id: &str) -> Result<ProviderSessionId, CliError> {
    ProviderSessionId::new(text(matches, id)?).map_err(|_| CliError::Arguments)
}
fn optional_provider(matches: &ArgMatches) -> Result<Option<ProviderId>, CliError> {
    matches
        .get_one::<String>("provider")
        .map(|value| ProviderId::new(value).map_err(|_| CliError::Arguments))
        .transpose()
}
fn optional_session(matches: &ArgMatches) -> Result<Option<ProviderSessionId>, CliError> {
    matches
        .get_one::<String>("session")
        .map(|value| ProviderSessionId::new(value).map_err(|_| CliError::Arguments))
        .transpose()
}
fn mailbox_selection(matches: &ArgMatches) -> Result<AgentMailboxSelection, CliError> {
    Ok(AgentMailboxSelection {
        provider: optional_provider(matches)?,
        session: optional_session(matches)?,
        directory: matches.get_one::<PathBuf>("directory").cloned(),
    })
}
fn optional_mailbox(matches: &ArgMatches, id: &str) -> Result<Option<MailboxId>, CliError> {
    matches
        .get_one::<String>(id)
        .map(|_| hex(matches, id).map(MailboxId::from_bytes))
        .transpose()
}
fn optional_installation(
    matches: &ArgMatches,
    id: &str,
) -> Result<Option<InstallationId>, CliError> {
    matches
        .get_one::<String>(id)
        .map(|_| hex(matches, id).map(InstallationId::from_bytes))
        .transpose()
}
fn relay(matches: &ArgMatches, id: &str) -> Result<super::RelayEndpoint, CliError> {
    relay_text(text(matches, id)?)
}
fn relay_text(value: &str) -> Result<super::RelayEndpoint, CliError> {
    super::RelayEndpoint::new(value.to_owned()).map_err(|_| CliError::Arguments)
}
fn normalized_path(matches: &ArgMatches, id: &str) -> Result<PathBuf, CliError> {
    let value = path(matches, id)?;
    let _ = super::normalized_existing_resource(&value)?;
    Ok(value)
}

fn pairing(matches: &ArgMatches) -> Result<(Option<ShortText>, RelayHints), CliError> {
    let label = optional_short_text(matches, "label")?;
    let mut relays = matches
        .get_many::<String>("relay")
        .into_iter()
        .flatten()
        .map(|value| {
            let text = BoundedText::<RESOURCE_LOCATOR_MAX_BYTES>::new(value)
                .map_err(|_| CliError::Arguments)?;
            Ok(ResourceLocator::new(ResourceScheme::Opaque, text))
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    relays.sort();
    if relays.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CliError::Arguments);
    }
    Ok((
        label,
        RelayHints::new(relays).map_err(|_| CliError::Arguments)?,
    ))
}

fn content(value: &str) -> Result<ContentText, CliError> {
    ContentText::new(value.to_owned()).map_err(|_| CliError::Arguments)
}

fn duration(value: &str) -> Result<Duration, CliError> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60_000)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3_600_000)
    } else {
        return Err(CliError::Arguments);
    };
    let milliseconds = number
        .parse::<u64>()
        .map_err(|_| CliError::Arguments)?
        .checked_mul(multiplier)
        .ok_or(CliError::Arguments)?;
    if milliseconds == 0 {
        return Err(CliError::Arguments);
    }
    Ok(Duration::from_millis(milliseconds))
}

fn agent_selector(value: &str) -> Result<NamedAgentSelector, CliError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(NamedAgentSelector::Id(AgentId::from_bytes(decode_hex32(
            value,
        )?)));
    }
    Ok(NamedAgentSelector::Name(validated_agent_name(value)?))
}

fn validated_agent_name(value: &str) -> Result<ShortText, CliError> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || !bytes[0].is_ascii_lowercase()
        || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(CliError::Arguments);
    }
    ShortText::new(value).map_err(|_| CliError::Arguments)
}

fn absolute(path: PathBuf) -> Result<PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(CliError::Arguments)
    }
}

fn decode_hex32(value: &str) -> Result<[u8; 32], CliError> {
    let value = value.as_bytes();
    if value.len() != 64 {
        return Err(CliError::Arguments);
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_chunks::<2>().0.iter().enumerate() {
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(output)
}

const fn hex_nibble(value: u8) -> Result<u8, CliError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(CliError::Arguments),
    }
}

pub(super) fn output_hint(arguments: &[OsString]) -> CliOutputFormat {
    for (index, argument) in arguments.iter().enumerate() {
        match argument.to_str() {
            Some("--output")
                if arguments
                    .get(index + 1)
                    .is_some_and(|value| value == "json") =>
            {
                return CliOutputFormat::Json;
            }
            Some("--output=json") => return CliOutputFormat::Json,
            _ => {}
        }
    }
    CliOutputFormat::Human
}

fn help_flag_topic(arguments: &[OsString]) -> Vec<String> {
    let mut topic = Vec::new();
    let mut current = command();
    let mut skip_value = false;
    for argument in arguments {
        if skip_value {
            skip_value = false;
            continue;
        }
        match argument.to_str() {
            Some("--output" | "--state-root") => {
                skip_value = true;
            }
            Some("--help" | "-h") => break,
            Some(value) if !value.starts_with('-') => {
                if let Some(next) = current.find_subcommand(value).cloned() {
                    topic.push(value.to_owned());
                    current = next;
                }
            }
            _ => {}
        }
    }
    topic
}
