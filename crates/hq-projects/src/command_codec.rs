//! Strict canonical remote-control body encoding.

use std::{error::Error, fmt, num::NonZeroU64};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hq_application::{ProjectCommandAction, WorktreeProvisioningRequest};
use hq_domain::{
    AccountId, AgentId, AssignmentBinding, AssignmentId, AssignmentIntent, BoundedText,
    CommandDigest, CommandId, ContentText, DispatchId, FactId, InstallationId, MessageId,
    OperationCorrelation, OperationId, ProjectId, ProjectResource, ProviderId, ProviderSessionId,
    ResourceHealth, ResourceId, ResourceLocator, ResourceScheme, ShortText, ThreadId, Timestamp,
};
use serde::{Deserialize, Serialize};

use crate::{CanonicalProjectMutation, CanonicalProjectMutationAction, PendingProjectInput};

const PREFIX: &str = "hq-project-command-v1:";
const CANONICAL_MUTATION_PREFIX: &str = "hq-project-canonical-mutation-v1:";
const MAX_CANONICAL_MUTATION_BYTES: usize = 65_536;

/// Failure to encode or strictly decode a remote project command body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectCommandCodecError {
    /// The body is malformed, noncanonical, or contains invalid typed values.
    Invalid,
    /// The body names an unsupported command codec version.
    UnsupportedVersion,
    /// The canonical body exceeds the domain content bound.
    TooLarge,
}

impl fmt::Display for ProjectCommandCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "project command body is invalid or noncanonical",
            Self::UnsupportedVersion => "project command body version is unsupported",
            Self::TooLarge => "project command body exceeds the content bound",
        })
    }
}

impl Error for ProjectCommandCodecError {}

/// Encodes one action into the only canonical remote-control v1 body spelling.
pub fn encode_project_command_action(
    action: &ProjectCommandAction,
) -> Result<ContentText, ProjectCommandCodecError> {
    let wire = WireAction::from(action);
    let json = serde_json::to_string(&wire).map_err(|_| ProjectCommandCodecError::Invalid)?;
    ContentText::new(format!("{PREFIX}{json}")).map_err(|_| ProjectCommandCodecError::TooLarge)
}

/// Strictly decodes one canonical remote-control v1 body.
pub fn decode_project_command_action(
    body: &ContentText,
) -> Result<ProjectCommandAction, ProjectCommandCodecError> {
    let Some(json) = body.as_str().strip_prefix(PREFIX) else {
        return Err(if body.as_str().starts_with("hq-project-command-v") {
            ProjectCommandCodecError::UnsupportedVersion
        } else {
            ProjectCommandCodecError::Invalid
        });
    };
    let wire: WireAction =
        serde_json::from_str(json).map_err(|_| ProjectCommandCodecError::Invalid)?;
    let canonical = serde_json::to_string(&wire).map_err(|_| ProjectCommandCodecError::Invalid)?;
    if canonical != json {
        return Err(ProjectCommandCodecError::Invalid);
    }
    ProjectCommandAction::try_from(wire)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum WireAction {
    Open,
    Activate {
        agent_id: String,
        provider: String,
        resume_session: Option<String>,
        resume_thread: Option<String>,
        launch_directory: WireLocator,
    },
    DispatchPending,
    Close {
        force: bool,
    },
    SetArchived {
        archived: bool,
    },
    Handoff {
        agent_id: String,
        provider: String,
        resume_session: Option<String>,
        thread_id: String,
        launch_directory: WireLocator,
        force_takeover: bool,
    },
    RetireAgent {
        agent_id: String,
        force: bool,
    },
    AddResource {
        resource: WireProjectResource,
        make_primary: bool,
    },
    RemoveResource {
        resource_id: String,
        force: bool,
    },
    ReplaceResource {
        old_resource_id: String,
        new_resource: WireProjectResource,
    },
    ProvisionWorktree {
        request: WireProvisioning,
    },
}

impl From<&ProjectCommandAction> for WireAction {
    fn from(action: &ProjectCommandAction) -> Self {
        match action {
            ProjectCommandAction::Open => Self::Open,
            ProjectCommandAction::Activate {
                agent_id,
                provider,
                resume_session,
                resume_thread,
                launch_directory,
            } => Self::Activate {
                agent_id: id_text(agent_id.as_bytes()),
                provider: provider.as_str().to_owned(),
                resume_session: resume_session
                    .as_ref()
                    .map(|session| session.as_str().to_owned()),
                resume_thread: resume_thread.map(|thread| id_text(thread.as_bytes())),
                launch_directory: WireLocator::from(launch_directory),
            },
            ProjectCommandAction::DispatchPending => Self::DispatchPending,
            ProjectCommandAction::Close { force } => Self::Close { force: *force },
            ProjectCommandAction::SetArchived { archived } => Self::SetArchived {
                archived: *archived,
            },
            ProjectCommandAction::Handoff {
                agent_id,
                provider,
                resume_session,
                thread_id,
                launch_directory,
                force_takeover,
            } => Self::Handoff {
                agent_id: id_text(agent_id.as_bytes()),
                provider: provider.as_str().to_owned(),
                resume_session: resume_session
                    .as_ref()
                    .map(|session| session.as_str().to_owned()),
                thread_id: id_text(thread_id.as_bytes()),
                launch_directory: WireLocator::from(launch_directory),
                force_takeover: *force_takeover,
            },
            ProjectCommandAction::RetireAgent { agent_id, force } => Self::RetireAgent {
                agent_id: id_text(agent_id.as_bytes()),
                force: *force,
            },
            ProjectCommandAction::AddResource {
                resource,
                make_primary,
            } => Self::AddResource {
                resource: WireProjectResource::from(resource),
                make_primary: *make_primary,
            },
            ProjectCommandAction::RemoveResource { resource_id, force } => Self::RemoveResource {
                resource_id: id_text(resource_id.as_bytes()),
                force: *force,
            },
            ProjectCommandAction::ReplaceResource {
                old_resource_id,
                new_resource,
            } => Self::ReplaceResource {
                old_resource_id: id_text(old_resource_id.as_bytes()),
                new_resource: WireProjectResource::from(new_resource),
            },
            ProjectCommandAction::ProvisionWorktree(request) => Self::ProvisionWorktree {
                request: WireProvisioning::from(request),
            },
        }
    }
}

impl TryFrom<WireAction> for ProjectCommandAction {
    type Error = ProjectCommandCodecError;

    fn try_from(action: WireAction) -> Result<Self, Self::Error> {
        Ok(match action {
            WireAction::Open => Self::Open,
            WireAction::Activate {
                agent_id,
                provider,
                resume_session,
                resume_thread,
                launch_directory,
            } => Self::Activate {
                agent_id: AgentId::from_bytes(parse_id(&agent_id)?),
                provider: ProviderId::new(provider).map_err(|_| Self::Error::Invalid)?,
                resume_session: resume_session
                    .map(ProviderSessionId::new)
                    .transpose()
                    .map_err(|_| Self::Error::Invalid)?,
                resume_thread: resume_thread
                    .map(|value| parse_id(&value).map(ThreadId::from_bytes))
                    .transpose()?,
                launch_directory: launch_directory.try_into()?,
            },
            WireAction::DispatchPending => Self::DispatchPending,
            WireAction::Close { force } => Self::Close { force },
            WireAction::SetArchived { archived } => Self::SetArchived { archived },
            WireAction::Handoff {
                agent_id,
                provider,
                resume_session,
                thread_id,
                launch_directory,
                force_takeover,
            } => Self::Handoff {
                agent_id: AgentId::from_bytes(parse_id(&agent_id)?),
                provider: ProviderId::new(provider).map_err(|_| Self::Error::Invalid)?,
                resume_session: resume_session
                    .map(ProviderSessionId::new)
                    .transpose()
                    .map_err(|_| Self::Error::Invalid)?,
                thread_id: ThreadId::from_bytes(parse_id(&thread_id)?),
                launch_directory: launch_directory.try_into()?,
                force_takeover,
            },
            WireAction::RetireAgent { agent_id, force } => Self::RetireAgent {
                agent_id: AgentId::from_bytes(parse_id(&agent_id)?),
                force,
            },
            WireAction::AddResource {
                resource,
                make_primary,
            } => Self::AddResource {
                resource: resource.try_into()?,
                make_primary,
            },
            WireAction::RemoveResource { resource_id, force } => Self::RemoveResource {
                resource_id: ResourceId::from_bytes(parse_id(&resource_id)?),
                force,
            },
            WireAction::ReplaceResource {
                old_resource_id,
                new_resource,
            } => Self::ReplaceResource {
                old_resource_id: ResourceId::from_bytes(parse_id(&old_resource_id)?),
                new_resource: new_resource.try_into()?,
            },
            WireAction::ProvisionWorktree { request } => {
                Self::ProvisionWorktree(request.try_into()?)
            }
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WireLocator {
    scheme: WireResourceScheme,
    value: String,
}

impl From<&ResourceLocator> for WireLocator {
    fn from(locator: &ResourceLocator) -> Self {
        Self {
            scheme: WireResourceScheme::from(locator.scheme()),
            value: locator.value().to_owned(),
        }
    }
}

impl TryFrom<WireLocator> for ResourceLocator {
    type Error = ProjectCommandCodecError;

    fn try_from(locator: WireLocator) -> Result<Self, Self::Error> {
        let value = BoundedText::new(locator.value).map_err(|_| Self::Error::Invalid)?;
        Ok(Self::new(locator.scheme.into(), value))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireResourceScheme {
    GitRepository,
    WorkingTree,
    Container,
    Opaque,
}

impl From<ResourceScheme> for WireResourceScheme {
    fn from(scheme: ResourceScheme) -> Self {
        match scheme {
            ResourceScheme::GitRepository => Self::GitRepository,
            ResourceScheme::WorkingTree => Self::WorkingTree,
            ResourceScheme::Container => Self::Container,
            ResourceScheme::Opaque => Self::Opaque,
        }
    }
}

impl From<WireResourceScheme> for ResourceScheme {
    fn from(scheme: WireResourceScheme) -> Self {
        match scheme {
            WireResourceScheme::GitRepository => Self::GitRepository,
            WireResourceScheme::WorkingTree => Self::WorkingTree,
            WireResourceScheme::Container => Self::Container,
            WireResourceScheme::Opaque => Self::Opaque,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WireProjectResource {
    resource_id: String,
    display_locator: WireLocator,
    canonical_locator: WireLocator,
    health: WireResourceHealth,
}

impl From<&ProjectResource> for WireProjectResource {
    fn from(resource: &ProjectResource) -> Self {
        Self {
            resource_id: id_text(resource.resource_id.as_bytes()),
            display_locator: WireLocator::from(&resource.display_locator),
            canonical_locator: WireLocator::from(&resource.canonical_locator),
            health: WireResourceHealth::from(resource.health),
        }
    }
}

impl TryFrom<WireProjectResource> for ProjectResource {
    type Error = ProjectCommandCodecError;

    fn try_from(resource: WireProjectResource) -> Result<Self, Self::Error> {
        Ok(Self {
            resource_id: ResourceId::from_bytes(parse_id(&resource.resource_id)?),
            display_locator: resource.display_locator.try_into()?,
            canonical_locator: resource.canonical_locator.try_into()?,
            health: resource.health.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireResourceHealth {
    Unknown,
    Healthy,
    Degraded,
    Unavailable,
}

impl From<ResourceHealth> for WireResourceHealth {
    fn from(health: ResourceHealth) -> Self {
        match health {
            ResourceHealth::Unknown => Self::Unknown,
            ResourceHealth::Healthy => Self::Healthy,
            ResourceHealth::Degraded => Self::Degraded,
            ResourceHealth::Unavailable => Self::Unavailable,
        }
    }
}

impl From<WireResourceHealth> for ResourceHealth {
    fn from(health: WireResourceHealth) -> Self {
        match health {
            WireResourceHealth::Unknown => Self::Unknown,
            WireResourceHealth::Healthy => Self::Healthy,
            WireResourceHealth::Degraded => Self::Degraded,
            WireResourceHealth::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WireProvisioning {
    mailbox_id: String,
    project_name: String,
    brief: Option<String>,
    source: WireLocator,
    destination: WireLocator,
    branch: String,
    create_branch: bool,
}

impl From<&WorktreeProvisioningRequest> for WireProvisioning {
    fn from(request: &WorktreeProvisioningRequest) -> Self {
        Self {
            mailbox_id: id_text(request.mailbox_id.as_bytes()),
            project_name: request.project_name.as_str().to_owned(),
            brief: request
                .brief
                .as_ref()
                .map(|brief| brief.as_str().to_owned()),
            source: WireLocator::from(&request.source),
            destination: WireLocator::from(&request.destination),
            branch: request.branch.as_str().to_owned(),
            create_branch: request.create_branch,
        }
    }
}

impl TryFrom<WireProvisioning> for WorktreeProvisioningRequest {
    type Error = ProjectCommandCodecError;

    fn try_from(request: WireProvisioning) -> Result<Self, Self::Error> {
        Ok(Self {
            mailbox_id: hq_domain::MailboxId::from_bytes(parse_id(&request.mailbox_id)?),
            project_name: ShortText::new(request.project_name).map_err(|_| Self::Error::Invalid)?,
            brief: request
                .brief
                .map(ContentText::new)
                .transpose()
                .map_err(|_| Self::Error::Invalid)?,
            source: request.source.try_into()?,
            destination: request.destination.try_into()?,
            branch: ShortText::new(request.branch).map_err(|_| Self::Error::Invalid)?,
            create_branch: request.create_branch,
        })
    }
}

/// Encodes one exact in-flight canonical mutation for durable saga reconciliation.
pub fn encode_canonical_project_mutation(
    mutation: &CanonicalProjectMutation,
) -> Result<Vec<u8>, ProjectCommandCodecError> {
    let json = serde_json::to_string(&WireCanonicalMutation::from(mutation))
        .map_err(|_| ProjectCommandCodecError::Invalid)?;
    let encoded = format!("{CANONICAL_MUTATION_PREFIX}{json}").into_bytes();
    if encoded.len() > MAX_CANONICAL_MUTATION_BYTES {
        return Err(ProjectCommandCodecError::TooLarge);
    }
    Ok(encoded)
}

/// Strictly decodes one exact in-flight canonical mutation after restart.
pub fn decode_canonical_project_mutation(
    encoded: &[u8],
) -> Result<CanonicalProjectMutation, ProjectCommandCodecError> {
    let text = std::str::from_utf8(encoded).map_err(|_| ProjectCommandCodecError::Invalid)?;
    let Some(json) = text.strip_prefix(CANONICAL_MUTATION_PREFIX) else {
        return Err(ProjectCommandCodecError::UnsupportedVersion);
    };
    let wire: WireCanonicalMutation =
        serde_json::from_str(json).map_err(|_| ProjectCommandCodecError::Invalid)?;
    if serde_json::to_string(&wire).map_err(|_| ProjectCommandCodecError::Invalid)? != json {
        return Err(ProjectCommandCodecError::Invalid);
    }
    wire.try_into()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WireCanonicalMutation {
    command_id: String,
    request_digest: String,
    account_id: String,
    project_id: String,
    home: String,
    expected_head: String,
    issued_at: i64,
    action: WireCanonicalAction,
}

impl From<&CanonicalProjectMutation> for WireCanonicalMutation {
    fn from(mutation: &CanonicalProjectMutation) -> Self {
        Self {
            command_id: id_text(mutation.command_id.as_bytes()),
            request_digest: id_text(mutation.request_digest.as_bytes()),
            account_id: id_text(mutation.account_id.as_bytes()),
            project_id: id_text(mutation.project_id.as_bytes()),
            home: id_text(mutation.home.as_bytes()),
            expected_head: id_text(mutation.expected_head.as_bytes()),
            issued_at: mutation.issued_at.as_unix_millis(),
            action: WireCanonicalAction::from(&mutation.action),
        }
    }
}

impl TryFrom<WireCanonicalMutation> for CanonicalProjectMutation {
    type Error = ProjectCommandCodecError;

    fn try_from(mutation: WireCanonicalMutation) -> Result<Self, Self::Error> {
        Ok(Self {
            command_id: CommandId::from_bytes(parse_id(&mutation.command_id)?),
            request_digest: CommandDigest::from_bytes(parse_id(&mutation.request_digest)?),
            account_id: AccountId::from_bytes(parse_id(&mutation.account_id)?),
            project_id: ProjectId::from_bytes(parse_id(&mutation.project_id)?),
            home: InstallationId::from_bytes(parse_id(&mutation.home)?),
            expected_head: FactId::from_bytes(parse_id(&mutation.expected_head)?),
            issued_at: Timestamp::from_unix_millis(mutation.issued_at),
            action: mutation.action.try_into()?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum WireCanonicalAction {
    Open,
    AddResource {
        resource: WireProjectResource,
        make_primary: bool,
    },
    RemoveResource {
        resource_id: String,
        force: bool,
    },
    ReplaceResource {
        old_resource_id: String,
        new_resource: WireProjectResource,
    },
    Configure {
        assignment: String,
        agent: String,
        provider: String,
    },
    MakeRunnable {
        binding: WireBinding,
        thread: String,
        launch_directory: WireLocator,
        activation: WireCorrelation,
    },
    EndAssignment {
        assignment: String,
    },
    BeginClosing,
    FinishClosing,
    RecordDispatch {
        input: WirePendingInput,
        dispatch: String,
        binding: WireBinding,
        thread: String,
    },
}

impl From<&CanonicalProjectMutationAction> for WireCanonicalAction {
    fn from(action: &CanonicalProjectMutationAction) -> Self {
        match action {
            CanonicalProjectMutationAction::Open => Self::Open,
            CanonicalProjectMutationAction::AddResource {
                resource,
                make_primary,
            } => Self::AddResource {
                resource: WireProjectResource::from(resource),
                make_primary: *make_primary,
            },
            CanonicalProjectMutationAction::RemoveResource { resource_id, force } => {
                Self::RemoveResource {
                    resource_id: id_text(resource_id.as_bytes()),
                    force: *force,
                }
            }
            CanonicalProjectMutationAction::ReplaceResource {
                old_resource_id,
                new_resource,
            } => Self::ReplaceResource {
                old_resource_id: id_text(old_resource_id.as_bytes()),
                new_resource: WireProjectResource::from(new_resource),
            },
            CanonicalProjectMutationAction::Configure(intent) => Self::Configure {
                assignment: id_text(intent.assignment_id.as_bytes()),
                agent: id_text(intent.agent_id.as_bytes()),
                provider: intent.provider.as_str().to_owned(),
            },
            CanonicalProjectMutationAction::MakeRunnable {
                binding,
                thread_id,
                launch_directory,
                activation,
            } => Self::MakeRunnable {
                binding: WireBinding::from(binding),
                thread: id_text(thread_id.as_bytes()),
                launch_directory: WireLocator::from(launch_directory),
                activation: WireCorrelation::from(activation),
            },
            CanonicalProjectMutationAction::EndAssignment { assignment_id } => {
                Self::EndAssignment {
                    assignment: id_text(assignment_id.as_bytes()),
                }
            }
            CanonicalProjectMutationAction::BeginClosing => Self::BeginClosing,
            CanonicalProjectMutationAction::FinishClosing => Self::FinishClosing,
            CanonicalProjectMutationAction::RecordDispatch {
                input,
                dispatch_id,
                binding,
                thread_id,
            } => Self::RecordDispatch {
                input: WirePendingInput::from(input),
                dispatch: id_text(dispatch_id.as_bytes()),
                binding: WireBinding::from(binding),
                thread: id_text(thread_id.as_bytes()),
            },
        }
    }
}

impl TryFrom<WireCanonicalAction> for CanonicalProjectMutationAction {
    type Error = ProjectCommandCodecError;

    fn try_from(action: WireCanonicalAction) -> Result<Self, Self::Error> {
        Ok(match action {
            WireCanonicalAction::Open => Self::Open,
            WireCanonicalAction::AddResource {
                resource,
                make_primary,
            } => Self::AddResource {
                resource: resource.try_into()?,
                make_primary,
            },
            WireCanonicalAction::RemoveResource { resource_id, force } => Self::RemoveResource {
                resource_id: ResourceId::from_bytes(parse_id(&resource_id)?),
                force,
            },
            WireCanonicalAction::ReplaceResource {
                old_resource_id,
                new_resource,
            } => Self::ReplaceResource {
                old_resource_id: ResourceId::from_bytes(parse_id(&old_resource_id)?),
                new_resource: new_resource.try_into()?,
            },
            WireCanonicalAction::Configure {
                assignment,
                agent,
                provider,
            } => Self::Configure(AssignmentIntent {
                assignment_id: AssignmentId::from_bytes(parse_id(&assignment)?),
                agent_id: AgentId::from_bytes(parse_id(&agent)?),
                provider: ProviderId::new(provider).map_err(|_| Self::Error::Invalid)?,
            }),
            WireCanonicalAction::MakeRunnable {
                binding,
                thread,
                launch_directory,
                activation,
            } => Self::MakeRunnable {
                binding: binding.try_into()?,
                thread_id: ThreadId::from_bytes(parse_id(&thread)?),
                launch_directory: launch_directory.try_into()?,
                activation: activation.try_into()?,
            },
            WireCanonicalAction::EndAssignment { assignment } => Self::EndAssignment {
                assignment_id: AssignmentId::from_bytes(parse_id(&assignment)?),
            },
            WireCanonicalAction::BeginClosing => Self::BeginClosing,
            WireCanonicalAction::FinishClosing => Self::FinishClosing,
            WireCanonicalAction::RecordDispatch {
                input,
                dispatch,
                binding,
                thread,
            } => Self::RecordDispatch {
                input: input.try_into()?,
                dispatch_id: DispatchId::from_bytes(parse_id(&dispatch)?),
                binding: binding.try_into()?,
                thread_id: ThreadId::from_bytes(parse_id(&thread)?),
            },
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WireBinding {
    assignment: String,
    agent: String,
    provider: String,
    session: String,
}

impl From<&AssignmentBinding> for WireBinding {
    fn from(binding: &AssignmentBinding) -> Self {
        Self {
            assignment: id_text(binding.assignment_id.as_bytes()),
            agent: id_text(binding.agent_id.as_bytes()),
            provider: binding.provider.as_str().to_owned(),
            session: binding.session.as_str().to_owned(),
        }
    }
}

impl TryFrom<WireBinding> for AssignmentBinding {
    type Error = ProjectCommandCodecError;
    fn try_from(binding: WireBinding) -> Result<Self, Self::Error> {
        Ok(Self {
            assignment_id: AssignmentId::from_bytes(parse_id(&binding.assignment)?),
            agent_id: AgentId::from_bytes(parse_id(&binding.agent)?),
            provider: ProviderId::new(binding.provider).map_err(|_| Self::Error::Invalid)?,
            session: ProviderSessionId::new(binding.session).map_err(|_| Self::Error::Invalid)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WireCorrelation {
    provider: String,
    session: String,
    operation: String,
}

impl From<&OperationCorrelation> for WireCorrelation {
    fn from(value: &OperationCorrelation) -> Self {
        Self {
            provider: value.provider().as_str().to_owned(),
            session: value.session().as_str().to_owned(),
            operation: id_text(value.operation().as_bytes()),
        }
    }
}

impl TryFrom<WireCorrelation> for OperationCorrelation {
    type Error = ProjectCommandCodecError;
    fn try_from(value: WireCorrelation) -> Result<Self, Self::Error> {
        Ok(Self::new(
            ProviderId::new(value.provider).map_err(|_| Self::Error::Invalid)?,
            ProviderSessionId::new(value.session).map_err(|_| Self::Error::Invalid)?,
            OperationId::from_bytes(parse_id(&value.operation)?),
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WirePendingInput {
    message: String,
    input_fact: String,
    accepted_fact: String,
    sequence: u64,
    thread: String,
    body: String,
}

impl From<&PendingProjectInput> for WirePendingInput {
    fn from(input: &PendingProjectInput) -> Self {
        Self {
            message: id_text(input.message_id.as_bytes()),
            input_fact: id_text(input.input_fact_id.as_bytes()),
            accepted_fact: id_text(input.accepted_fact.as_bytes()),
            sequence: input.sequence.get(),
            thread: id_text(input.thread_id.as_bytes()),
            body: input.body.as_str().to_owned(),
        }
    }
}

impl TryFrom<WirePendingInput> for PendingProjectInput {
    type Error = ProjectCommandCodecError;
    fn try_from(input: WirePendingInput) -> Result<Self, Self::Error> {
        Ok(Self {
            message_id: MessageId::from_bytes(parse_id(&input.message)?),
            input_fact_id: FactId::from_bytes(parse_id(&input.input_fact)?),
            accepted_fact: FactId::from_bytes(parse_id(&input.accepted_fact)?),
            sequence: NonZeroU64::new(input.sequence).ok_or(Self::Error::Invalid)?,
            thread_id: ThreadId::from_bytes(parse_id(&input.thread)?),
            body: ContentText::new(input.body).map_err(|_| Self::Error::Invalid)?,
        })
    }
}

fn id_text(bytes: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn parse_id(value: &str) -> Result<[u8; 32], ProjectCommandCodecError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ProjectCommandCodecError::Invalid)?
        .try_into()
        .map_err(|_| ProjectCommandCodecError::Invalid)
}
