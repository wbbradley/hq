//! Bounded blocking Unix transport for the pure reconnecting local API client.

use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    io::Read as _,
    num::NonZeroUsize,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use hq_domain::{
    AgentId, ContentText, InstallationId, ProjectId, ProviderId, ProviderSessionId, ShortText,
    ThreadId,
};
use hq_local_api::{
    BlockingClientConfig, BlockingClientError, BlockingClientRunner, ClientConnectionState,
    ClientEvent, ClientTransport, InitialView, ReconnectPolicy, ReconnectingClient,
    protocol::v1::{
        AgentRetirementRequestDto, AgentSessionRequestDto, AuthoritativeSnapshotDto, BuildMetadata,
        EffectRequestDto, FrameDecoder, Id32, InvalidationTopic, MailboxCommandRequestDto,
        MutationRequest, ProjectCommandRequestDto, Request,
    },
};

use crate::{
    LifecycleClient, LifecycleClientConfig, NodeClientCoordinator, NodeCoordinatorConfig,
    NodeCoordinatorError, NodeLaunchError, NodeLauncher, ProcessNodeLauncher, RuntimePathError,
    RuntimePaths, StatePaths,
    cli::{
        CliError, HarnessCommand, NamedAgentCommand, NamedAgentSelector, NamedAgentView,
        ProjectCliCommand, ProjectResourceCliCommand, ProjectTuiResult, WorktreeCliRequest,
        named_agent_catalog_view, preview_project_creation_resource_for_tui,
        preview_project_resource_for_tui, project_catalog_for_tui, run_harness_for_tui,
        run_named_agent_for_tui, run_project_for_tui,
    },
    unix_frame,
};

/// Passive named-agent command accepted by the ordinary local client composition.
pub(crate) enum LocalNamedAgentCommand {
    Create {
        name: String,
    },
    RenameSession {
        agent_id: [u8; 32],
        provider: String,
        session: String,
        display_name: Option<String>,
    },
    Retire {
        agent_id: [u8; 32],
        force: bool,
    },
}

pub(crate) fn execute_named_agent_command(
    state: &StatePaths,
    command: LocalNamedAgentCommand,
) -> Result<u64, CliError> {
    let command = match command {
        LocalNamedAgentCommand::Create { name } => NamedAgentCommand::Create {
            name: ShortText::new(name).map_err(|_| CliError::Arguments)?,
            mailbox_id: None,
        },
        LocalNamedAgentCommand::RenameSession {
            agent_id,
            provider,
            session,
            display_name,
        } => NamedAgentCommand::Rename {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(agent_id)),
            provider: Some(ProviderId::new(provider).map_err(|_| CliError::Arguments)?),
            session: Some(ProviderSessionId::new(session).map_err(|_| CliError::Arguments)?),
            display_name: display_name
                .map(ShortText::new)
                .transpose()
                .map_err(|_| CliError::Arguments)?,
        },
        LocalNamedAgentCommand::Retire { agent_id, force } => NamedAgentCommand::Retire {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(agent_id)),
            force,
        },
    };
    run_named_agent_for_tui(&command, state)
}

pub(crate) fn tui_named_agent_catalog(snapshot: &AuthoritativeSnapshotDto) -> Vec<NamedAgentView> {
    named_agent_catalog_view(snapshot, "tui_snapshot", None, None).agents
}

/// Passive managed-session command accepted by the ordinary local client composition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalManagedSessionCommand {
    Start {
        agent_id: [u8; 32],
        provider: String,
    },
    Resume {
        agent_id: [u8; 32],
        provider: String,
        session: String,
    },
    Stop {
        agent_id: [u8; 32],
        provider: String,
    },
}

/// Typed managed-session outcome returned across the passive local-client seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalManagedSessionOutcome {
    Ready { session: String },
    Stopped,
    Rejected { category: String, code: String },
    Uncertain { reconciliation_id: [u8; 32] },
}

/// Passive completion evidence for one stable managed-session operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalManagedSessionResult {
    pub command: LocalManagedSessionCommand,
    pub operation_id: [u8; 32],
    pub outcome: LocalManagedSessionOutcome,
}

pub(crate) fn execute_managed_session_command(
    state: &StatePaths,
    command: LocalManagedSessionCommand,
) -> Result<LocalManagedSessionResult, CliError> {
    let action = match &command {
        LocalManagedSessionCommand::Start { agent_id, provider } => HarnessCommand::Start {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
            directory: None,
        },
        LocalManagedSessionCommand::Resume {
            agent_id,
            provider,
            session,
        } => HarnessCommand::Resume {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
            session: ProviderSessionId::new(session.clone()).map_err(|_| CliError::Arguments)?,
            directory: None,
        },
        LocalManagedSessionCommand::Stop { agent_id, provider } => HarnessCommand::Stop {
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
        },
    };
    let view = run_harness_for_tui(&action, state)?;
    let outcome = match view.status {
        "ready" => LocalManagedSessionOutcome::Ready {
            session: view.ready_session.ok_or(CliError::HarnessState)?,
        },
        "stopped" => LocalManagedSessionOutcome::Stopped,
        "rejected" => LocalManagedSessionOutcome::Rejected {
            category: view.error_category.ok_or(CliError::HarnessState)?,
            code: view.error_code.ok_or(CliError::HarnessState)?,
        },
        "uncertain" => LocalManagedSessionOutcome::Uncertain {
            reconciliation_id: view.reconciliation_id.ok_or(CliError::HarnessState)?,
        },
        _ => return Err(CliError::HarnessState),
    };
    Ok(LocalManagedSessionResult {
        command,
        operation_id: *view.operation_id.as_bytes(),
        outcome,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProjectResource {
    pub resource_id: [u8; 32],
    pub display_path: String,
    pub canonical_path: String,
    pub health: String,
    pub primary: bool,
    pub active_claim: bool,
    pub conflicting_projects: Vec<[u8; 32]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProjectAssignment {
    pub assignment_id: [u8; 32],
    pub agent_id: [u8; 32],
    pub provider: String,
    pub session: Option<String>,
    pub phase: String,
    pub thread_id: Option<[u8; 32]>,
    pub launch_directory: Option<String>,
    pub blocked: Option<String>,
    pub cardinality_conflicted: bool,
    pub runnable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProjectThread {
    pub agent_id: [u8; 32],
    pub provider: String,
    pub session: String,
    pub thread_id: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalPendingProjectInput {
    pub message_id: [u8; 32],
    pub thread_id: [u8; 32],
    pub sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProject {
    pub project_id: [u8; 32],
    pub home: [u8; 32],
    pub name: String,
    pub lifecycle: String,
    pub archived: bool,
    pub claimable: bool,
    pub assignment: Option<LocalProjectAssignment>,
    pub threads: Vec<LocalProjectThread>,
    pub pending_inputs: Vec<LocalPendingProjectInput>,
    pub head: [u8; 32],
    pub input_sequence: u64,
    pub resources: Vec<LocalProjectResource>,
}

pub(crate) fn tui_project_catalog(
    snapshot: &AuthoritativeSnapshotDto,
) -> Result<Vec<LocalProject>, CliError> {
    Ok(project_catalog_for_tui(snapshot)?
        .projects
        .into_iter()
        .map(|project| {
            let dispatched = project
                .dispatches
                .iter()
                .map(|dispatch| dispatch.message_id)
                .collect::<std::collections::BTreeSet<_>>();
            let pending_inputs = project
                .inputs
                .iter()
                .filter(|input| !dispatched.contains(&input.message_id))
                .map(|input| LocalPendingProjectInput {
                    message_id: *input.message_id.as_bytes(),
                    thread_id: *input.thread_id.as_bytes(),
                    sequence: input.sequence,
                })
                .collect();
            LocalProject {
                project_id: *project.project_id.as_bytes(),
                home: *project.home.as_bytes(),
                name: project.name,
                lifecycle: project.lifecycle,
                archived: project.archived,
                claimable: project.claimable,
                assignment: project.assignment.map(|assignment| LocalProjectAssignment {
                    assignment_id: *assignment.assignment_id.as_bytes(),
                    agent_id: *assignment.agent_id.as_bytes(),
                    provider: assignment.provider,
                    session: assignment.session,
                    phase: assignment.phase,
                    thread_id: assignment.thread_id.map(|thread| *thread.as_bytes()),
                    launch_directory: assignment.launch_directory.map(|path| path.value),
                    blocked: assignment.blocked,
                    cardinality_conflicted: assignment.cardinality_conflicted,
                    runnable: assignment.runnable,
                }),
                threads: project
                    .threads
                    .into_iter()
                    .map(|thread| LocalProjectThread {
                        agent_id: *thread.agent_id.as_bytes(),
                        provider: thread.provider,
                        session: thread.session,
                        thread_id: *thread.thread_id.as_bytes(),
                    })
                    .collect(),
                pending_inputs,
                head: *project.head.as_bytes(),
                input_sequence: project.input_sequence,
                resources: project
                    .resources
                    .into_iter()
                    .map(|resource| LocalProjectResource {
                        resource_id: *resource.resource_id.as_bytes(),
                        display_path: resource.display_locator.value,
                        canonical_path: resource.canonical_locator.value,
                        health: resource.health.to_owned(),
                        primary: resource.primary,
                        active_claim: resource.active_claim,
                        conflicting_projects: resource
                            .conflicting_projects
                            .into_iter()
                            .map(|project_id| *project_id.as_bytes())
                            .collect(),
                    })
                    .collect(),
            }
        })
        .collect())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalProjectCommand {
    PreviewCreateExisting {
        name: String,
        brief: Option<String>,
        path: String,
    },
    CreateExisting {
        name: String,
        brief: Option<String>,
        path: String,
    },
    CreateWorktree {
        name: String,
        brief: Option<String>,
        source: String,
        destination: String,
        branch: String,
        base: Option<String>,
    },
    PreviewAddResource {
        project_id: [u8; 32],
        path: String,
        make_primary: bool,
    },
    AddResource {
        project_id: [u8; 32],
        path: String,
        make_primary: bool,
    },
    PreviewReplaceResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
        path: String,
    },
    ReplaceResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
        path: String,
    },
    RemoveResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
        force: bool,
    },
    SetPrimaryResource {
        project_id: [u8; 32],
        resource_id: [u8; 32],
    },
    CheckResources {
        project_id: [u8; 32],
        resource_id: Option<[u8; 32]>,
    },
    Activate {
        project_id: [u8; 32],
        agent_id: [u8; 32],
        provider: String,
        resume_session: Option<String>,
        resume_thread: Option<[u8; 32]>,
        launch_directory: String,
    },
    DispatchPending {
        project_id: [u8; 32],
    },
    Handoff {
        project_id: [u8; 32],
        agent_id: [u8; 32],
        provider: String,
        resume_session: Option<String>,
        thread_id: [u8; 32],
        launch_directory: String,
        force_takeover: bool,
    },
    Open {
        project_id: [u8; 32],
    },
    PreviewClose {
        project_id: [u8; 32],
    },
    Close {
        project_id: [u8; 32],
        force: bool,
    },
    SetArchived {
        project_id: [u8; 32],
        archived: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProjectExternalWarning {
    pub kind: String,
    pub destination: String,
    pub branch: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProjectResourceConflict {
    pub project_id: [u8; 32],
    pub resource_id: [u8; 32],
    pub display_path: String,
    pub canonical_path: String,
    pub relationship: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProjectResourceCheck {
    pub resource_id: [u8; 32],
    pub status: String,
    pub health: Option<String>,
    pub release: Option<String>,
    pub observed_canonical_path: Option<String>,
    pub details: Option<String>,
    pub error_category: Option<String>,
    pub error_code: Option<String>,
    pub reconciliation_id: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalProjectOutcome {
    Completed {
        project_head: Option<[u8; 32]>,
    },
    Running {
        stage: String,
    },
    Rejected {
        category: String,
        code: String,
    },
    Reconcilable {
        stage: String,
        category: String,
        code: String,
        warning: Option<LocalProjectExternalWarning>,
    },
    InputSent {
        message_id: [u8; 32],
    },
    ResourcePreview {
        display_path: String,
        canonical_path: String,
        conflicts: Vec<LocalProjectResourceConflict>,
    },
    ResourceChecks {
        checks: Vec<LocalProjectResourceCheck>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalProjectResult {
    pub command: LocalProjectCommand,
    pub command_id: [u8; 32],
    pub operation_id: [u8; 32],
    pub project_id: [u8; 32],
    pub runtime_state: Option<String>,
    pub runtime_code: Option<String>,
    pub outcome: LocalProjectOutcome,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn execute_project_command(
    state: &StatePaths,
    command: LocalProjectCommand,
) -> Result<LocalProjectResult, CliError> {
    if let Some(preview) = execute_project_preview(state, &command)? {
        return Ok(preview);
    }
    let action = match &command {
        LocalProjectCommand::CreateExisting { name, brief, path } => ProjectCliCommand::Create {
            name: ShortText::new(name.clone()).map_err(|_| CliError::Arguments)?,
            brief: brief
                .clone()
                .map(ContentText::new)
                .transpose()
                .map_err(|_| CliError::Arguments)?,
            path: PathBuf::from(path),
            home: None,
        },
        LocalProjectCommand::CreateWorktree {
            name,
            brief,
            source,
            destination,
            branch,
            base,
        } => ProjectCliCommand::Worktree(WorktreeCliRequest {
            name: ShortText::new(name.clone()).map_err(|_| CliError::Arguments)?,
            brief: brief
                .clone()
                .map(ContentText::new)
                .transpose()
                .map_err(|_| CliError::Arguments)?,
            source: PathBuf::from(source),
            destination: PathBuf::from(destination),
            branch: ShortText::new(branch.clone()).map_err(|_| CliError::Arguments)?,
            base: base
                .clone()
                .map(ShortText::new)
                .transpose()
                .map_err(|_| CliError::Arguments)?,
            home: None,
        }),
        LocalProjectCommand::AddResource {
            project_id,
            path,
            make_primary,
        } => ProjectCliCommand::Resource(ProjectResourceCliCommand::Add {
            project_id: ProjectId::from_bytes(*project_id),
            path: PathBuf::from(path),
            make_primary: *make_primary,
        }),
        LocalProjectCommand::ReplaceResource {
            project_id,
            resource_id,
            path,
        } => ProjectCliCommand::Resource(ProjectResourceCliCommand::Replace {
            project_id: ProjectId::from_bytes(*project_id),
            resource_id: hq_domain::ResourceId::from_bytes(*resource_id),
            path: PathBuf::from(path),
        }),
        LocalProjectCommand::RemoveResource {
            project_id,
            resource_id,
            force,
        } => ProjectCliCommand::Resource(ProjectResourceCliCommand::Remove {
            project_id: ProjectId::from_bytes(*project_id),
            resource_id: hq_domain::ResourceId::from_bytes(*resource_id),
            force: *force,
        }),
        LocalProjectCommand::SetPrimaryResource {
            project_id,
            resource_id,
        } => ProjectCliCommand::Resource(ProjectResourceCliCommand::Primary {
            project_id: ProjectId::from_bytes(*project_id),
            resource_id: hq_domain::ResourceId::from_bytes(*resource_id),
        }),
        LocalProjectCommand::CheckResources {
            project_id,
            resource_id,
        } => ProjectCliCommand::Check {
            project_id: ProjectId::from_bytes(*project_id),
            resource_id: resource_id.map(hq_domain::ResourceId::from_bytes),
        },
        LocalProjectCommand::Activate {
            project_id,
            agent_id,
            provider,
            resume_session,
            resume_thread,
            launch_directory,
        } => ProjectCliCommand::Activate {
            project_id: ProjectId::from_bytes(*project_id),
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
            resume_session: resume_session
                .clone()
                .map(ProviderSessionId::new)
                .transpose()
                .map_err(|_| CliError::Arguments)?,
            resume_thread: resume_thread.map(ThreadId::from_bytes),
            directory: Some(PathBuf::from(launch_directory)),
        },
        LocalProjectCommand::DispatchPending { project_id } => {
            ProjectCliCommand::Dispatch(ProjectId::from_bytes(*project_id))
        }
        LocalProjectCommand::Handoff {
            project_id,
            agent_id,
            provider,
            resume_session,
            thread_id,
            launch_directory,
            force_takeover,
        } => ProjectCliCommand::Handoff {
            project_id: ProjectId::from_bytes(*project_id),
            agent: NamedAgentSelector::Id(AgentId::from_bytes(*agent_id)),
            provider: ProviderId::new(provider.clone()).map_err(|_| CliError::Arguments)?,
            resume_session: resume_session
                .clone()
                .map(ProviderSessionId::new)
                .transpose()
                .map_err(|_| CliError::Arguments)?,
            thread_id: ThreadId::from_bytes(*thread_id),
            directory: Some(PathBuf::from(launch_directory)),
            force: *force_takeover,
        },
        LocalProjectCommand::Open { project_id } => {
            ProjectCliCommand::Open(ProjectId::from_bytes(*project_id))
        }
        LocalProjectCommand::PreviewClose { project_id } => ProjectCliCommand::Check {
            project_id: ProjectId::from_bytes(*project_id),
            resource_id: None,
        },
        LocalProjectCommand::Close { project_id, force } => ProjectCliCommand::Close {
            project_id: ProjectId::from_bytes(*project_id),
            force: *force,
        },
        LocalProjectCommand::SetArchived {
            project_id,
            archived,
        } => {
            let project_id = ProjectId::from_bytes(*project_id);
            if *archived {
                ProjectCliCommand::Archive(project_id)
            } else {
                ProjectCliCommand::Unarchive(project_id)
            }
        }
        LocalProjectCommand::PreviewCreateExisting { .. }
        | LocalProjectCommand::PreviewAddResource { .. }
        | LocalProjectCommand::PreviewReplaceResource { .. } => {
            unreachable!("preview commands return before command conversion")
        }
    };
    project_result(command, run_project_for_tui(&action, state)?)
}

fn execute_project_preview(
    state: &StatePaths,
    command: &LocalProjectCommand,
) -> Result<Option<LocalProjectResult>, CliError> {
    if let LocalProjectCommand::PreviewCreateExisting { path, .. } = command {
        let preview = preview_project_creation_resource_for_tui(state, &PathBuf::from(path))?;
        return Ok(Some(local_project_preview_result(command, preview)));
    }
    let (project_id, path) = match command {
        LocalProjectCommand::PreviewAddResource {
            project_id, path, ..
        }
        | LocalProjectCommand::PreviewReplaceResource {
            project_id, path, ..
        } => (*project_id, path),
        _ => return Ok(None),
    };
    let preview = preview_project_resource_for_tui(
        state,
        ProjectId::from_bytes(project_id),
        &PathBuf::from(path),
    )?;
    Ok(Some(local_project_preview_result(command, preview)))
}

fn local_project_preview_result(
    command: &LocalProjectCommand,
    preview: crate::cli::ProjectResourcePreviewView,
) -> LocalProjectResult {
    let operation_id = *preview.operation_id.as_bytes();
    LocalProjectResult {
        command: command.clone(),
        command_id: operation_id,
        operation_id,
        project_id: *preview.project_id.as_bytes(),
        runtime_state: None,
        runtime_code: None,
        outcome: LocalProjectOutcome::ResourcePreview {
            display_path: preview.display_path,
            canonical_path: preview.canonical_path,
            conflicts: preview
                .conflicts
                .into_iter()
                .map(|conflict| LocalProjectResourceConflict {
                    project_id: *conflict.project_id.as_bytes(),
                    resource_id: *conflict.resource_id.as_bytes(),
                    display_path: conflict.display_path,
                    canonical_path: conflict.canonical_path,
                    relationship: conflict.relationship.to_owned(),
                })
                .collect(),
        },
    }
}

fn project_result(
    command: LocalProjectCommand,
    result: ProjectTuiResult,
) -> Result<LocalProjectResult, CliError> {
    match result {
        ProjectTuiResult::Operation(view) => {
            let runtime_state = view.runtime_state.map(str::to_owned);
            let runtime_code = view.runtime_code;
            let outcome = match view.status {
                "accepted" | "running" => LocalProjectOutcome::Running {
                    stage: view.stage.ok_or(CliError::ProjectState)?.to_owned(),
                },
                "completed" => LocalProjectOutcome::Completed {
                    project_head: view.project_head.map(|head| *head.as_bytes()),
                },
                "rejected" => LocalProjectOutcome::Rejected {
                    category: view.error_category.ok_or(CliError::ProjectState)?,
                    code: view.error_code.ok_or(CliError::ProjectState)?,
                },
                "reconcilable" => LocalProjectOutcome::Reconcilable {
                    stage: view.stage.ok_or(CliError::ProjectState)?.to_owned(),
                    category: view.error_category.ok_or(CliError::ProjectState)?,
                    code: view.error_code.ok_or(CliError::ProjectState)?,
                    warning: view.external_state_warning.map(|warning| {
                        LocalProjectExternalWarning {
                            kind: warning.kind.to_owned(),
                            destination: warning.destination,
                            branch: warning.branch,
                        }
                    }),
                },
                _ => return Err(CliError::ProjectState),
            };
            Ok(LocalProjectResult {
                command,
                command_id: *view.command_id.as_bytes(),
                operation_id: *view.operation_id.as_bytes(),
                project_id: *view.project_id.as_bytes(),
                runtime_state,
                runtime_code,
                outcome,
            })
        }
        ProjectTuiResult::InputSent {
            project_id,
            message_id,
        } => Ok(LocalProjectResult {
            command,
            command_id: *message_id.as_bytes(),
            operation_id: *message_id.as_bytes(),
            project_id: *project_id.as_bytes(),
            runtime_state: None,
            runtime_code: None,
            outcome: LocalProjectOutcome::InputSent {
                message_id: *message_id.as_bytes(),
            },
        }),
        ProjectTuiResult::ResourceChecks(view) => {
            let operation_id = view
                .checks
                .first()
                .map_or([0; 32], |check| *check.operation_id.as_bytes());
            Ok(LocalProjectResult {
                command,
                command_id: operation_id,
                operation_id,
                project_id: *view.project_id.as_bytes(),
                runtime_state: None,
                runtime_code: None,
                outcome: LocalProjectOutcome::ResourceChecks {
                    checks: view
                        .checks
                        .into_iter()
                        .map(|check| LocalProjectResourceCheck {
                            resource_id: *check.resource_id.as_bytes(),
                            status: check.status.to_owned(),
                            health: check.health.map(str::to_owned),
                            release: check.release.map(str::to_owned),
                            observed_canonical_path: check
                                .observed_canonical
                                .map(|locator| locator.value),
                            details: check.details,
                            error_category: check.error_category,
                            error_code: check.error_code,
                            reconciliation_id: check
                                .reconciliation_id
                                .map(|operation_id| *operation_id.as_bytes()),
                        })
                        .collect(),
                },
            })
        }
    }
}

/// Passive local Unix transport configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnixClientTransportConfig {
    /// Exact private runtime namespace containing the node socket.
    pub runtime: RuntimePaths,
    /// Positive bound independently applied to every socket read and write.
    pub io_timeout: Duration,
}

/// Closed local transport failure without operating-system prose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnixClientTransportError {
    /// The configured I/O timeout was zero.
    InvalidTimeout,
    /// No local node listener accepted the connection.
    Absent,
    /// Socket setup, read, or write failed.
    Transport,
    /// A frame length exceeded the protocol bound.
    Protocol,
}

impl fmt::Display for UnixClientTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local client transport failed: {self:?}")
    }
}

impl Error for UnixClientTransportError {}

/// Plain bounds and identities for one reusable command client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalNodeClientConfig {
    /// Validated installation state layout used for readiness and runtime derivation.
    pub state: StatePaths,
    /// Safe build metadata sent during protocol negotiation.
    pub build: BuildMetadata,
    /// Whether this client needs an initial authoritative state view.
    pub initial_view: InitialView,
    /// Positive timeout independently applied to every socket read and write.
    pub io_timeout: Duration,
    /// Inclusive wall-time bound for each typed command execution.
    pub command_deadline: Duration,
    /// Maximum connection attempts for each typed command execution.
    pub max_connection_attempts: NonZeroUsize,
    /// Maximum time allowed for absent-node autostart convergence.
    pub readiness_timeout: Duration,
    /// Positive coordinator polling interval.
    pub readiness_retry_interval: Duration,
    /// Initial positive reconnect delay after connection loss.
    pub reconnect_initial: Duration,
    /// Inclusive maximum reconnect delay.
    pub reconnect_maximum: Duration,
    /// Maximum retained completed retry-safe command identities.
    pub completed_identity_capacity: NonZeroUsize,
}

pub(crate) const fn installed_local_client_config(
    state: StatePaths,
    build: BuildMetadata,
    initial_view: InitialView,
) -> LocalNodeClientConfig {
    LocalNodeClientConfig {
        state,
        build,
        initial_view,
        io_timeout: Duration::from_secs(2),
        command_deadline: Duration::from_secs(10),
        max_connection_attempts: positive_usize(8),
        readiness_timeout: Duration::from_secs(10),
        readiness_retry_interval: Duration::from_millis(25),
        reconnect_initial: Duration::from_millis(25),
        reconnect_maximum: Duration::from_millis(250),
        completed_identity_capacity: positive_usize(64),
    }
}

const fn positive_usize(value: usize) -> NonZeroUsize {
    match NonZeroUsize::new(value) {
        Some(value) => value,
        None => unreachable!(),
    }
}

/// Closed command-client setup or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalNodeClientError {
    /// The private runtime path could not be derived.
    RuntimePath,
    /// Node readiness or autostart did not converge.
    Coordinator(NodeCoordinatorError),
    /// The current executable could not be resolved for autostart.
    Launcher(NodeLaunchError),
    /// The Unix transport configuration was invalid.
    Transport(UnixClientTransportError),
    /// Reconnect policy or client state construction failed.
    Client,
    /// A bounded request execution failed.
    Execution(BlockingClientError),
}

impl fmt::Display for LocalNodeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local node client failed: {self:?}")
    }
}

impl Error for LocalNodeClientError {}

/// Reusable typed local command seam with no direct signer, storage, relay, or provider access.
pub struct LocalNodeClient {
    installation_id: InstallationId,
    runner: BlockingClientRunner<UnixClientTransport>,
}

/// Long-lived subscribed local client for interactive event-driven frontends.
pub struct LocalNodeEventClient {
    installation_id: InstallationId,
    runner: BlockingClientRunner<UnixClientTransport>,
    interrupt: UnixClientInterrupt,
    observation_deadline: Duration,
}

#[derive(Clone, Copy)]
enum SubscriptionMode {
    None,
    All,
}

impl LocalNodeClient {
    /// Converges readiness through the installed executable and opens a bounded command client.
    pub fn connect(config: LocalNodeClientConfig) -> Result<Self, LocalNodeClientError> {
        let launcher =
            ProcessNodeLauncher::current_executable().map_err(LocalNodeClientError::Launcher)?;
        Self::connect_with_launcher(config, launcher)
    }

    /// Converges readiness through an injected launcher before opening the local API transport.
    pub fn connect_with_launcher<L: NodeLauncher>(
        config: LocalNodeClientConfig,
        launcher: L,
    ) -> Result<Self, LocalNodeClientError> {
        let (installation_id, runner, _interrupt) =
            connect_runner(config, launcher, SubscriptionMode::None)?;
        Ok(Self {
            installation_id,
            runner,
        })
    }

    /// Returns the installation authenticated by coordinator readiness.
    pub const fn installation_id(&self) -> InstallationId {
        self.installation_id
    }

    /// Executes one non-retryable typed request.
    pub fn request(&mut self, request: Request) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .request(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Loads one fresh complete authoritative snapshot.
    pub fn snapshot(&mut self) -> Result<AuthoritativeSnapshotDto, LocalNodeClientError> {
        self.runner
            .snapshot()
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe fact mutation.
    pub fn mutation(
        &mut self,
        request: MutationRequest,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .mutation(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe authoritative mailbox command.
    pub fn mailbox_command(
        &mut self,
        request: MailboxCommandRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .mailbox_command(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe durable project command.
    pub fn project(
        &mut self,
        request: ProjectCommandRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .project(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe node-owned named-agent retirement.
    pub fn agent_retirement(
        &mut self,
        request: AgentRetirementRequestDto,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .agent_retirement(request)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Executes or reconciles one retry-safe managed named-agent session operation.
    pub fn agent_session(
        &mut self,
        request: EffectRequestDto<AgentSessionRequestDto>,
    ) -> Result<ClientEvent, LocalNodeClientError> {
        self.runner
            .agent_session(request)
            .map_err(LocalNodeClientError::Execution)
    }
}

impl LocalNodeEventClient {
    /// Converges readiness and opens a broad-invalidation subscribed local client.
    pub fn connect(config: LocalNodeClientConfig) -> Result<Self, LocalNodeClientError> {
        let launcher =
            ProcessNodeLauncher::current_executable().map_err(LocalNodeClientError::Launcher)?;
        Self::connect_with_launcher(config, launcher)
    }

    /// Converges readiness through an injected launcher before subscribing to all revisions.
    pub fn connect_with_launcher<L: NodeLauncher>(
        config: LocalNodeClientConfig,
        launcher: L,
    ) -> Result<Self, LocalNodeClientError> {
        let observation_deadline = config.command_deadline;
        let (installation_id, runner, interrupt) =
            connect_runner(config, launcher, SubscriptionMode::All)?;
        Ok(Self {
            installation_id,
            runner,
            interrupt,
            observation_deadline,
        })
    }

    /// Returns the installation authenticated by coordinator readiness.
    pub const fn installation_id(&self) -> InstallationId {
        self.installation_id
    }

    /// Drives subscribed work until an event, connection-state change, or workflow deadline.
    pub fn next_observation(&mut self) -> Result<Option<ClientEvent>, LocalNodeClientError> {
        self.runner
            .poll_event_or_state_change(self.observation_deadline)
            .map_err(LocalNodeClientError::Execution)
    }

    /// Completes the initial subscription acknowledgement and returns its authoritative base.
    pub fn activate_subscription(
        &mut self,
    ) -> Result<AuthoritativeSnapshotDto, LocalNodeClientError> {
        loop {
            if let Some(event) = self.next_observation()? {
                match event {
                    ClientEvent::Snapshot(snapshot) => return Ok(snapshot),
                    ClientEvent::AuthoritativeConversationView(view) => return Ok(view.snapshot),
                    ClientEvent::IncompatibleVersion => {
                        return Err(LocalNodeClientError::Execution(
                            BlockingClientError::Incompatible,
                        ));
                    }
                    ClientEvent::Error { .. } => {
                        return Err(LocalNodeClientError::Client);
                    }
                    ClientEvent::Mutation(_)
                    | ClientEvent::ProjectCommand { .. }
                    | ClientEvent::AgentRetirement { .. }
                    | ClientEvent::AgentSession { .. }
                    | ClientEvent::Response { .. }
                    | ClientEvent::RequestLost(_) => {}
                }
            }
        }
    }

    /// Returns a cloneable handle that wakes the active subscribed Unix read.
    pub fn interrupt_handle(&self) -> UnixClientInterrupt {
        self.interrupt.clone()
    }

    /// Returns the generation-scoped reconnecting-client state.
    pub const fn connection_state(&self) -> ClientConnectionState {
        self.runner.connection_state()
    }
}

fn connect_runner<L: NodeLauncher>(
    config: LocalNodeClientConfig,
    launcher: L,
    subscription: SubscriptionMode,
) -> Result<
    (
        InstallationId,
        BlockingClientRunner<UnixClientTransport>,
        UnixClientInterrupt,
    ),
    LocalNodeClientError,
> {
    let runtime = RuntimePaths::new(config.state.root().join("runtime"))
        .map_err(|_error: RuntimePathError| LocalNodeClientError::RuntimePath)?;
    let probe = LifecycleClient::new(LifecycleClientConfig {
        runtime: runtime.clone(),
        build: config.build.clone(),
        io_timeout: config.io_timeout,
    })
    .map_err(|_| LocalNodeClientError::Client)?;
    let mut coordinator = NodeClientCoordinator::new(
        probe,
        launcher,
        NodeCoordinatorConfig {
            state_root: config.state.root().to_path_buf(),
            readiness_timeout: config.readiness_timeout,
            retry_interval: config.readiness_retry_interval,
        },
    )
    .map_err(LocalNodeClientError::Coordinator)?;
    let ready = coordinator
        .ensure_ready()
        .map_err(LocalNodeClientError::Coordinator)?;
    let installation_id = ready
        .observation
        .readiness
        .as_ref()
        .map(|readiness| InstallationId::from_bytes(readiness.installation_id.bytes()))
        .ok_or(LocalNodeClientError::Client)?;
    let transport = UnixClientTransport::new(UnixClientTransportConfig {
        runtime,
        io_timeout: config.io_timeout,
    })
    .map_err(LocalNodeClientError::Transport)?;
    let interrupt = transport.interrupt_handle();
    let reconnect = ReconnectPolicy::new(config.reconnect_initial, config.reconnect_maximum)
        .map_err(|_| LocalNodeClientError::Client)?;
    let mut client = ReconnectingClient::new(
        config.build,
        reconnect,
        config.completed_identity_capacity.get(),
        config.initial_view,
    )
    .map_err(|_| LocalNodeClientError::Client)?;
    if matches!(subscription, SubscriptionMode::All) {
        client
            .configure_subscription(
                Id32::new(*installation_id.as_bytes()),
                vec![InvalidationTopic::All],
            )
            .map_err(|_| LocalNodeClientError::Client)?;
    }
    let runner = BlockingClientRunner::new(
        BlockingClientConfig {
            deadline: config.command_deadline,
            max_connection_attempts: config.max_connection_attempts,
        },
        client,
        transport,
    )
    .map_err(LocalNodeClientError::Execution)?;
    Ok((installation_id, runner, interrupt))
}

/// Standard blocking transport that owns no state beyond validated configuration.
#[derive(Clone, Debug)]
pub struct UnixClientTransport {
    config: UnixClientTransportConfig,
    interrupt: UnixClientInterrupt,
    next_connection_id: Arc<AtomicU64>,
}

/// Cloneable capability that interrupts only the transport's currently active Unix connection.
#[derive(Clone, Debug, Default)]
pub struct UnixClientInterrupt {
    active: Arc<Mutex<Option<InterruptConnection>>>,
}

#[derive(Debug)]
struct InterruptConnection {
    id: u64,
    stream: UnixStream,
}

impl UnixClientInterrupt {
    /// Wakes an idle transport read without waiting for its configured timeout.
    pub fn interrupt(&self) {
        if let Ok(active) = self.active.lock()
            && let Some(active) = active.as_ref()
        {
            let _ = active.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    fn register(&self, id: u64, stream: UnixStream) {
        if let Ok(mut active) = self.active.lock() {
            *active = Some(InterruptConnection { id, stream });
        }
    }

    fn clear(&self, id: u64) {
        if let Ok(mut active) = self.active.lock()
            && active.as_ref().is_some_and(|active| active.id == id)
        {
            *active = None;
        }
    }
}

/// One Unix client connection with an incremental frame decoder retained across idle polls.
#[derive(Debug)]
pub struct UnixClientConnection {
    id: u64,
    stream: UnixStream,
    decoder: FrameDecoder,
    ready_frames: VecDeque<Vec<u8>>,
}

impl UnixClientConnection {
    #[cfg(test)]
    fn new(stream: UnixStream) -> Self {
        Self {
            id: 0,
            stream,
            decoder: FrameDecoder::new(),
            ready_frames: VecDeque::new(),
        }
    }

    fn tracked(id: u64, stream: UnixStream) -> Self {
        Self {
            id,
            stream,
            decoder: FrameDecoder::new(),
            ready_frames: VecDeque::new(),
        }
    }
}

impl UnixClientTransport {
    /// Validates and retains one local Unix transport configuration.
    pub fn new(config: UnixClientTransportConfig) -> Result<Self, UnixClientTransportError> {
        if config.io_timeout.is_zero() {
            return Err(UnixClientTransportError::InvalidTimeout);
        }
        Ok(Self {
            config,
            interrupt: UnixClientInterrupt::default(),
            next_connection_id: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Returns an idempotent handle for waking the currently active connection.
    pub fn interrupt_handle(&self) -> UnixClientInterrupt {
        self.interrupt.clone()
    }

    fn own_connection(
        &self,
        stream: UnixStream,
    ) -> Result<UnixClientConnection, UnixClientTransportError> {
        let id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            return Err(UnixClientTransportError::Transport);
        }
        let interrupt_stream = stream
            .try_clone()
            .map_err(|_| UnixClientTransportError::Transport)?;
        self.interrupt.register(id, interrupt_stream);
        Ok(UnixClientConnection::tracked(id, stream))
    }
}

impl ClientTransport for UnixClientTransport {
    type Connection = UnixClientConnection;
    type Error = UnixClientTransportError;

    fn connect(&mut self) -> Result<Self::Connection, Self::Error> {
        let stream = UnixStream::connect(self.config.runtime.socket_file()).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                UnixClientTransportError::Absent
            } else {
                UnixClientTransportError::Transport
            }
        })?;
        stream
            .set_read_timeout(Some(self.config.io_timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        self.own_connection(stream)
    }

    fn write(
        &mut self,
        connection: &mut Self::Connection,
        frame: &[u8],
        timeout: Duration,
    ) -> Result<(), Self::Error> {
        connection
            .stream
            .set_write_timeout(Some(timeout.min(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        unix_frame::write_frame(&mut connection.stream, frame)
            .map_err(|_| UnixClientTransportError::Transport)
    }

    fn read_frame(
        &mut self,
        connection: &mut Self::Connection,
        timeout: Duration,
    ) -> Result<Vec<u8>, Self::Error> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(UnixClientTransportError::Transport)?;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(UnixClientTransportError::Transport)?;
            if let Some(frame) = self.poll_frame(connection, remaining)? {
                return Ok(frame);
            }
        }
    }

    fn poll_frame(
        &mut self,
        connection: &mut Self::Connection,
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        if let Some(frame) = connection.ready_frames.pop_front() {
            return Ok(Some(frame));
        }
        if timeout.is_zero() {
            return Ok(None);
        }
        connection
            .stream
            .set_read_timeout(Some(timeout.min(self.config.io_timeout)))
            .map_err(|_| UnixClientTransportError::Transport)?;
        let mut bytes = [0_u8; 8_192];
        let count = match connection.stream.read(&mut bytes) {
            Ok(0) if connection.decoder.buffered_len() == 0 => {
                return Err(UnixClientTransportError::Transport);
            }
            Ok(0) => return Err(UnixClientTransportError::Protocol),
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(_) => return Err(UnixClientTransportError::Transport),
        };
        let mut next = connection.decoder.push(&bytes[..count]);
        loop {
            let message = match next {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(_) => return Err(UnixClientTransportError::Protocol),
            };
            connection.ready_frames.push_back(
                message
                    .encode_frame()
                    .map_err(|_| UnixClientTransportError::Protocol)?,
            );
            next = connection.decoder.push(&[]);
        }
        Ok(connection.ready_frames.pop_front())
    }

    fn close(&mut self, connection: Self::Connection) {
        self.interrupt.clear(connection.id);
        let _ = connection.stream.shutdown(std::net::Shutdown::Both);
    }

    fn wait(&mut self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        io::{Read as _, Write as _},
        os::unix::net::UnixStream,
        thread,
        time::Duration,
    };

    use hq_local_api::{
        ClientTransport,
        protocol::v1::{BuildMetadata, Id32, ServerHello, V1, WireMessage},
    };

    use super::{UnixClientConnection, UnixClientTransport, UnixClientTransportConfig};
    use crate::RuntimePaths;

    #[test]
    fn unix_poll_preserves_a_partial_frame_across_an_idle_timeout() {
        let (mut writer, reader) = UnixStream::pair().expect("socket pair");
        let mut connection = UnixClientConnection::new(reader);
        let mut transport = UnixClientTransport::new(UnixClientTransportConfig {
            runtime: RuntimePaths::new(
                std::env::temp_dir().join("hq-local-client-partial-frame-test"),
            )
            .expect("absolute runtime path"),
            io_timeout: Duration::from_millis(10),
        })
        .expect("transport");
        let frame = WireMessage::ServerHello(ServerHello::new(
            V1,
            BuildMetadata::new("hq-test", "0.1.0", None::<String>).expect("build"),
            Id32::new([7; 32]),
        ))
        .encode_frame()
        .expect("frame");

        writer.write_all(&frame[..2]).expect("partial prefix");
        assert_eq!(
            transport
                .poll_frame(&mut connection, Duration::from_millis(1))
                .expect("idle partial poll"),
            None
        );
        writer.write_all(&frame[2..]).expect("remaining frame");
        assert_eq!(
            transport
                .poll_frame(&mut connection, Duration::from_millis(10))
                .expect("completed poll"),
            Some(frame)
        );
    }

    #[test]
    fn unix_interrupt_wakes_an_idle_read_and_old_close_cannot_disarm_the_new_generation() {
        let mut transport = UnixClientTransport::new(UnixClientTransportConfig {
            runtime: RuntimePaths::new(std::env::temp_dir().join("hq-local-client-interrupt-test"))
                .expect("absolute runtime path"),
            io_timeout: Duration::from_secs(30),
        })
        .expect("transport");
        let interrupt = transport.interrupt_handle();
        let (_first_writer, first_reader) = UnixStream::pair().expect("first socket pair");
        let first = transport.own_connection(first_reader).expect("first owner");
        let (_second_writer, second_reader) = UnixStream::pair().expect("second socket pair");
        let mut second = transport
            .own_connection(second_reader)
            .expect("second owner");

        transport.close(first);
        let reader = thread::spawn(move || {
            let mut byte = [0_u8; 1];
            second.stream.read(&mut byte)
        });
        interrupt.interrupt();

        let result = reader.join().expect("idle reader joined");
        assert!(matches!(result, Ok(0) | Err(_)));
    }
}
