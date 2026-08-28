//! Complete catalog payload fixtures.

use std::num::NonZeroU64;

use hq_domain::{
    ActivityKind, AssignmentBinding, BoundedText, BoundedVec, ContentText, ErrorCode,
    InitialProjectState, InstallationAddress, MailboxAddress, MailboxKind, MessageContent,
    MessagePurpose, OperationCorrelation, PresentationKind, ProjectResource, ProviderId,
    ProviderSessionId, RemoteCommandResult, RepositoryContext, ResourceHealth, ResourceLocator,
    ResourceScheme, RuntimeObservation, SemanticPayload, ShortText, Timestamp,
};

use crate::{DeterministicValues, FactBuilder, FixtureError};

impl FactBuilder {
    /// Instantiates every catalog payload in stable FCT order.
    #[allow(clippy::too_many_lines)]
    pub fn all_catalog_payloads(
        values: &mut DeterministicValues,
    ) -> Result<Vec<SemanticPayload>, FixtureError> {
        let text = |value| ShortText::new(value);
        let content_text = |value| ContentText::new(value);
        let locator = |value| {
            BoundedText::<4_096>::new(value)
                .map(|value| ResourceLocator::new(ResourceScheme::WorkingTree, value))
        };
        let provider = ProviderId::new("test-provider")?;
        let session = ProviderSessionId::new("session-1")?;
        let installation_id = values.installation_id();
        let signing_key = values.signing_key();
        let author = InstallationAddress::new(installation_id, signing_key);
        let mailbox = MailboxAddress::new(installation_id, values.mailbox_id());
        let account_id = hq_domain::AccountId::from_bytes(values.bytes());
        let agent_id = values.agent_id();
        let project_id = values.project_id();
        let message_id = values.message_id();
        let resource = ProjectResource {
            resource_id: values.resource_id(),
            display_locator: locator("/work/project")?,
            canonical_locator: locator("/work/project")?,
            health: ResourceHealth::Healthy,
        };
        let operation =
            OperationCorrelation::new(provider.clone(), session.clone(), values.operation_id());
        let context = RepositoryContext {
            directory: locator("/work/project")?,
            repository: Some(locator("/work")?),
            worktree: Some(locator("/work/project")?),
            branch: Some(text("main")?),
        };
        let assignment = AssignmentBinding {
            assignment_id: values.assignment_id(),
            agent_id,
            provider: provider.clone(),
            session: session.clone(),
        };
        let message = MessageContent {
            message_id,
            sender: mailbox,
            recipient: None,
            body: content_text("message")?,
            purpose: MessagePurpose::Question,
            presentation: PresentationKind::Message,
            correlation: Some(operation.clone()),
            project_id: Some(project_id),
        };
        let asynchronous = MessageContent {
            purpose: MessagePurpose::Asynchronous,
            ..message.clone()
        };
        let output = MessageContent {
            purpose: MessagePurpose::ProjectOutput,
            ..message.clone()
        };
        let grant_id = values.grant_id();
        let command_id = values.command_id();
        let digest = values.command_digest();
        let expected_head = values.fact_id();
        let assignment_id = assignment.assignment_id;
        let resource_id = resource.resource_id;
        let thread_id = values.thread_id();
        let dispatch_id = values.dispatch_id();
        let empty_hints = || BoundedVec::new([]);

        Ok(vec![
            SemanticPayload::InstallationDeclared {
                installation_id,
                signing_key,
                encryption_key: values.encryption_key(),
                label: Some(text("home")?),
            },
            SemanticPayload::MailboxCreated {
                mailbox_id: mailbox.mailbox_id(),
                kind: MailboxKind::Agent,
                label: Some(text("agent")?),
            },
            SemanticPayload::MailboxSessionBound {
                mailbox_id: mailbox.mailbox_id(),
                provider: provider.clone(),
                session: session.clone(),
            },
            SemanticPayload::MailboxContextRecorded {
                mailbox_id: mailbox.mailbox_id(),
                context: context.clone(),
            },
            SemanticPayload::PeerRouteSet {
                peer: author,
                encryption_key: values.encryption_key(),
                label: Some(text("peer")?),
                relay_hints: empty_hints()?,
            },
            SemanticPayload::PeerRouteBlocked {
                peer_id: installation_id,
                reason: ErrorCode::new("blocked")?,
            },
            SemanticPayload::MailboxAccessGranted {
                grant_id,
                mailbox,
                grantee: author,
            },
            SemanticPayload::MailboxAccessRevoked {
                grant_id,
                mailbox,
                grantee_id: installation_id,
            },
            SemanticPayload::MailboxActionObserved {
                grant_id,
                action_id: values.fact_id(),
            },
            SemanticPayload::HumanAccountCreated {
                account_id,
                creator: author,
                label: Some(text("account")?),
            },
            SemanticPayload::HumanAccountSelected { account_id },
            SemanticPayload::HumanDeviceGranted {
                account_id,
                grant_id,
                device: author,
                label: Some(text("device")?),
                relay_hints: empty_hints()?,
            },
            SemanticPayload::HumanDeviceAccepted {
                account_id,
                grant_id,
                device: author,
            },
            SemanticPayload::HumanDeviceRevoked {
                account_id,
                grant_id,
                device_id: installation_id,
            },
            SemanticPayload::QuestionAsked(message.clone()),
            SemanticPayload::AsynchronousMessageSent(asynchronous),
            SemanticPayload::AnswerGiven {
                thread_id,
                message: message.clone(),
            },
            SemanticPayload::ThreadCancelled {
                thread_id,
                reason: Some(content_text("cancelled")?),
            },
            SemanticPayload::MessageArchived { message_id },
            SemanticPayload::MessageRestored { message_id },
            SemanticPayload::MessageRejected {
                message_id,
                reason: ErrorCode::new("rejected")?,
            },
            SemanticPayload::HarnessActivityRecorded {
                source: mailbox,
                correlation: operation.clone(),
                item: Some(text("item")?),
                kind: ActivityKind::Progress,
                logical_key: text("build")?,
                runtime: text("runtime")?,
                sequence: NonZeroU64::MIN,
                occurred_at: Timestamp::from_unix_millis(1),
                status: hq_domain::ActivityStatus::Running,
                content: content_text("running")?,
                truncated: false,
            },
            SemanticPayload::AgentNameClaimed {
                agent_id,
                mailbox_id: mailbox.mailbox_id(),
                name: text("agent")?,
            },
            SemanticPayload::AgentRetired {
                agent_id,
                mailbox_id: mailbox.mailbox_id(),
            },
            SemanticPayload::ProviderSessionSelected {
                agent_id,
                mailbox_id: mailbox.mailbox_id(),
                provider: provider.clone(),
                session: session.clone(),
                context: context.clone(),
            },
            SemanticPayload::ProviderSessionRenamed {
                agent_id,
                provider: provider.clone(),
                session: session.clone(),
                display_name: Some(text("session")?),
            },
            SemanticPayload::ProjectCreated {
                project_id,
                mailbox_id: mailbox.mailbox_id(),
                home: installation_id,
                name: text("project")?,
                brief: Some(content_text("brief")?),
                predecessor: None,
                resources: BoundedVec::new([resource.clone()])?,
                primary: Some(resource_id),
                initial_state: InitialProjectState::Open,
            },
            SemanticPayload::ProjectOpened { project_id },
            SemanticPayload::ProjectClosingStarted { project_id },
            SemanticPayload::ProjectClosed {
                project_id,
                forced: false,
                runtime: Some(RuntimeObservation::Succeeded),
            },
            SemanticPayload::ProjectArchived { project_id },
            SemanticPayload::ProjectUnarchived { project_id },
            SemanticPayload::ProjectMetadataUpdated {
                project_id,
                name: text("renamed")?,
                brief: Some(content_text("updated")?),
            },
            SemanticPayload::ProjectResourceAdded {
                project_id,
                resource: resource.clone(),
                make_primary: true,
            },
            SemanticPayload::ProjectResourceRemoved {
                project_id,
                resource_id,
                force: false,
            },
            SemanticPayload::ProjectResourceReplaced {
                project_id,
                old_resource_id: resource_id,
                new_resource: ProjectResource {
                    resource_id: values.resource_id(),
                    display_locator: locator("/work/replacement")?,
                    canonical_locator: locator("/work/replacement")?,
                    health: ResourceHealth::Unknown,
                },
            },
            SemanticPayload::ProjectPrimaryResourceChanged {
                project_id,
                resource_id,
            },
            SemanticPayload::ProjectResourceHealthObserved {
                project_id,
                resource_id,
                health: ResourceHealth::Healthy,
                details: Some(content_text("healthy")?),
                checked_at: Timestamp::from_unix_millis(10),
            },
            SemanticPayload::ProjectAssignmentConfiguring {
                project_id,
                intent: hq_domain::AssignmentIntent {
                    assignment_id: assignment.assignment_id,
                    agent_id: assignment.agent_id,
                    provider: assignment.provider.clone(),
                },
            },
            SemanticPayload::ProjectAssignmentRunnable {
                project_id,
                binding: assignment.clone(),
                thread_id,
                launch_directory: locator("/work/project")?,
                activation: operation.clone(),
            },
            SemanticPayload::ProjectAssignmentBlocked {
                project_id,
                assignment_id,
                cause: ErrorCode::new("provider-offline")?,
            },
            SemanticPayload::ProjectAssignmentEnded {
                project_id,
                assignment_id,
                forced: false,
                runtime: Some(RuntimeObservation::Succeeded),
            },
            SemanticPayload::ProjectInputAccepted {
                project_id,
                message_id,
                input_fact_id: values.fact_id(),
                sequence: NonZeroU64::MIN,
            },
            SemanticPayload::ProjectInputDispatched {
                project_id,
                message_id,
                sequence: NonZeroU64::MIN,
                dispatch_id,
                binding: assignment.clone(),
                thread_id,
            },
            SemanticPayload::ProjectOutputRecorded {
                project_id,
                output_id: values.message_id(),
                dispatch_id,
                binding: assignment,
                thread_id,
                message: output,
            },
            SemanticPayload::RemoteProjectCommandRequested {
                command_id,
                digest,
                project_id,
                target_home: installation_id,
                expected_head: Some(expected_head),
                operation,
                body: content_text("open")?,
            },
            SemanticPayload::RemoteProjectCommandReceipt {
                command_id,
                digest,
                project_id,
                received_head: Some(expected_head),
                received_at: Timestamp::from_unix_millis(11),
            },
            SemanticPayload::RemoteProjectCommandOutcome {
                command_id,
                digest,
                project_id,
                result: RemoteCommandResult::Committed(values.fact_id()),
                runtime: Some(RuntimeObservation::Succeeded),
            },
        ])
    }
}
