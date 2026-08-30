//! Exhaustive conversion from verified wire DTOs into domain facts.

use std::fmt;

use hq_domain as domain;

use super::{VerifiedSupportedRecord, model};
use crate::{CryptographicallyVerifiedEvent, FailureClass, ProtocolError, ProtocolNamespace};

/// A semantic fact together with the exact cryptographically verified record that produced it.
pub struct VerifiedSemanticFact {
    record: VerifiedSupportedRecord,
    fact: domain::SemanticFact,
}

impl VerifiedSemanticFact {
    /// Returns the independently versioned signed-content namespace.
    pub const fn namespace(&self) -> ProtocolNamespace {
        self.record.namespace()
    }

    /// Returns the exact supported v1 family number.
    pub const fn family(&self) -> u64 {
        self.record.family()
    }

    /// Returns the fully validated semantic fact.
    pub const fn fact(&self) -> &domain::SemanticFact {
        &self.fact
    }

    /// Returns the exact retained cryptographic evidence.
    pub const fn verified_event(&self) -> &CryptographicallyVerifiedEvent {
        self.record.verified_event()
    }

    /// Returns the exact retained signed content bytes.
    pub fn content_bytes(&self) -> &[u8] {
        self.record.content_bytes()
    }

    /// Separates the semantic fact from its complete verified DTO record.
    pub fn into_parts(self) -> (domain::SemanticFact, VerifiedSupportedRecord) {
        (self.fact, self.record)
    }
}

impl fmt::Debug for VerifiedSemanticFact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedSemanticFact")
            .field("fact", &self.fact)
            .field("record", &self.record)
            .finish()
    }
}

pub(super) fn convert(
    record: VerifiedSupportedRecord,
) -> Result<VerifiedSemanticFact, ProtocolError> {
    let dto = &record.dto;
    let author = domain::InstallationAddress::new(
        domain::InstallationId::from_bytes(dto.author.0),
        domain::SigningPublicKey::from_bytes(record.verified_event().public_key()),
    );
    let scope = scope(&dto.scope);
    let causal = causal(&dto.parents, &dto.authorities)?;
    let payload = payload(&dto.body, author, &scope)?;
    let authored_at = domain::Timestamp::from_unix_millis(
        i64::try_from(dto.time.0).map_err(|_| failure(FailureClass::DomainValueInvalid))?,
    );
    let fact = domain::SemanticFact::new(
        domain::FactId::from_bytes(record.verified_event().event_id()),
        author,
        authored_at,
        scope,
        causal,
        payload,
    )
    .map_err(|error| match error {
        domain::SemanticFactError::ProtocolScopeMismatch => {
            failure(FailureClass::ScopePayloadMismatch)
        }
        domain::SemanticFactError::PayloadInvariant => failure(FailureClass::PayloadInvariant),
        domain::SemanticFactError::AuthorSubjectMismatch => {
            failure(FailureClass::AuthorSubjectMismatch)
        }
    })?;
    Ok(VerifiedSemanticFact { record, fact })
}

pub(super) fn plan(dto: &model::ContentDto) -> Result<super::CanonicalEventPlan, ProtocolError> {
    let signing_key = match &dto.body {
        model::BodyDto::InstallationDeclared(value) => signing(value.signing),
        model::BodyDto::HumanAccountCreated(value) => signing(value.creator.signing),
        model::BodyDto::HumanDeviceAccepted(value) => signing(value.device.signing),
        _ => domain::SigningPublicKey::from_bytes([0; 32]),
    };
    let author = domain::InstallationAddress::new(
        domain::InstallationId::from_bytes(dto.author.0),
        signing_key,
    );
    let scope = scope(&dto.scope);
    let causal = causal(&dto.parents, &dto.authorities)?;
    let payload = payload(&dto.body, author, &scope)?;
    let authored_at = domain::Timestamp::from_unix_millis(
        i64::try_from(dto.time.0).map_err(|_| failure(FailureClass::DomainValueInvalid))?,
    );
    Ok(super::CanonicalEventPlan::new(
        author.installation_id(),
        authored_at,
        scope,
        causal,
        payload,
    ))
}

fn scope(value: &model::ScopeDto) -> domain::FactScope {
    match value {
        model::ScopeDto::Local((_, installation)) => domain::FactScope::InstallationPrivate(
            domain::InstallationId::from_bytes(installation.0),
        ),
        model::ScopeDto::Peer((_, installation, mailbox)) => {
            domain::FactScope::PeerAddressed(domain::MailboxAddress::new(
                domain::InstallationId::from_bytes(installation.0),
                domain::MailboxId::from_bytes(mailbox.0),
            ))
        }
        model::ScopeDto::Account((_, account)) => {
            domain::FactScope::AccountAddressed(domain::AccountId::from_bytes(account.0))
        }
        model::ScopeDto::Control((_, account, target_home)) => domain::FactScope::RemoteControl {
            account_id: domain::AccountId::from_bytes(account.0),
            target_home: domain::InstallationId::from_bytes(target_home.0),
        },
    }
}

fn causal(
    parents: &[model::ParentDto],
    authorities: &[model::AuthorityDto],
) -> Result<
    domain::CausalReferences<{ domain::MAX_FACT_PARENTS }, { domain::MAX_FACT_AUTHORITIES }>,
    ProtocolError,
> {
    let parents = domain::BoundedSet::new(
        parents
            .iter()
            .map(|parent| domain::FactId::from_bytes(parent.1.0)),
    )
    .map_err(domain_value)?;
    let authorities = authorities.iter().map(|authority| {
        domain::AuthorityReference::new(
            authority_role(authority.0),
            domain::FactId::from_bytes(authority.2.0),
        )
    });
    domain::CausalReferences::new(parents, authorities).map_err(domain_value)
}

const fn authority_role(value: model::RoleDto) -> domain::AuthorityRole {
    match value {
        model::RoleDto::AccountCreator => domain::AuthorityRole::AccountCreator,
        model::RoleDto::AccountMembership => domain::AuthorityRole::AccountMembership,
        model::RoleDto::ActiveHuman => domain::AuthorityRole::ActiveHuman,
        model::RoleDto::Assignment => domain::AuthorityRole::Assignment,
        model::RoleDto::DeviceGrant => domain::AuthorityRole::DeviceGrant,
        model::RoleDto::Dispatch => domain::AuthorityRole::Dispatch,
        model::RoleDto::LocalInstallation => domain::AuthorityRole::LocalInstallation,
        model::RoleDto::MailboxGrant => domain::AuthorityRole::MailboxGrant,
        model::RoleDto::MailboxOwner => domain::AuthorityRole::MailboxOwner,
        model::RoleDto::OutputBinding => domain::AuthorityRole::OutputBinding,
        model::RoleDto::PreviousState => domain::AuthorityRole::PreviousState,
        model::RoleDto::ProjectHome => domain::AuthorityRole::ProjectHome,
        model::RoleDto::Request => domain::AuthorityRole::Request,
    }
}

#[allow(clippy::too_many_lines)]
fn payload(
    body: &model::BodyDto,
    author: domain::InstallationAddress,
    scope: &domain::FactScope,
) -> Result<domain::SemanticPayload, ProtocolError> {
    use domain::SemanticPayload as Output;

    Ok(match body {
        model::BodyDto::InstallationDeclared(value) => Output::InstallationDeclared {
            installation_id: installation(value.installation),
            signing_key: signing(value.signing),
            encryption_key: domain::EncryptionPublicKey::from_bytes(value.encryption.0),
            label: optional_short(&value.label)?,
        },
        model::BodyDto::MailboxCreated(value) => Output::MailboxCreated {
            mailbox_id: mailbox(value.mailbox),
            kind: mailbox_kind(value.kind),
            label: optional_short(&value.label)?,
        },
        model::BodyDto::MailboxSessionBound(value) => Output::MailboxSessionBound {
            mailbox_id: mailbox(value.mailbox),
            provider: provider(&value.provider)?,
            session: session(&value.session)?,
        },
        model::BodyDto::MailboxContextRecorded(value) => Output::MailboxContextRecorded {
            mailbox_id: mailbox(value.mailbox),
            context: context(&value.context)?,
        },
        model::BodyDto::PeerRouteSet(value) => {
            let peer = installation_address(&value.peer);
            ensure(
                peer.installation_id() != author.installation_id(),
                FailureClass::AuthorSubjectMismatch,
            )?;
            Output::PeerRouteSet {
                peer,
                encryption_key: domain::EncryptionPublicKey::from_bytes(value.encryption.0),
                label: optional_short(&value.label)?,
                relay_hints: domain::BoundedVec::new(
                    value
                        .relays
                        .iter()
                        .map(locator)
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .map_err(domain_value)?,
            }
        }
        model::BodyDto::PeerRouteBlocked(value) => Output::PeerRouteBlocked {
            peer_id: installation(value.peer),
            reason: error_code(&value.reason)?,
        },
        model::BodyDto::MailboxAccessGranted(value) => {
            require_mailbox_scope(scope, &value.mailbox)?;
            Output::MailboxAccessGranted {
                grant_id: domain::GrantId::from_bytes(value.grant.0),
                mailbox: mailbox_address(&value.mailbox),
                grantee: installation_address(&value.grantee),
            }
        }
        model::BodyDto::MailboxAccessRevoked(value) => {
            require_mailbox_scope(scope, &value.mailbox)?;
            Output::MailboxAccessRevoked {
                grant_id: domain::GrantId::from_bytes(value.grant.0),
                mailbox: mailbox_address(&value.mailbox),
                grantee_id: installation(value.grantee),
            }
        }
        model::BodyDto::MailboxActionObserved(value) => Output::MailboxActionObserved {
            grant_id: domain::GrantId::from_bytes(value.grant.0),
            action_id: domain::FactId::from_bytes(value.action.0),
        },
        model::BodyDto::HumanAccountCreated(value) => {
            let creator = installation_address(&value.creator);
            ensure(creator == author, FailureClass::AuthorSubjectMismatch)?;
            Output::HumanAccountCreated {
                account_id: account(value.account),
                creator,
                label: optional_short(&value.label)?,
            }
        }
        model::BodyDto::HumanAccountSelected(value) => Output::HumanAccountSelected {
            account_id: account(value.account),
        },
        model::BodyDto::HumanDeviceGranted(value) => {
            require_account_scope(scope, value.account)?;
            Output::HumanDeviceGranted {
                account_id: account(value.account),
                grant_id: domain::GrantId::from_bytes(value.grant.0),
                device: installation_address(&value.device),
                label: optional_short(&value.label)?,
                relay_hints: domain::BoundedVec::new(
                    value
                        .relays
                        .iter()
                        .map(locator)
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .map_err(domain_value)?,
            }
        }
        model::BodyDto::HumanDeviceAccepted(value) => {
            require_account_scope(scope, value.account)?;
            let device = installation_address(&value.device);
            ensure(device == author, FailureClass::AuthorSubjectMismatch)?;
            Output::HumanDeviceAccepted {
                account_id: account(value.account),
                grant_id: domain::GrantId::from_bytes(value.grant.0),
                device,
            }
        }
        model::BodyDto::HumanDeviceRevoked(value) => {
            require_account_scope(scope, value.account)?;
            Output::HumanDeviceRevoked {
                account_id: account(value.account),
                grant_id: domain::GrantId::from_bytes(value.grant.0),
                device_id: installation(value.device),
            }
        }
        model::BodyDto::QuestionAsked(value) => {
            Output::QuestionAsked(message(value, author, scope, false)?)
        }
        model::BodyDto::AsynchronousMessageSent(value) => {
            Output::AsynchronousMessageSent(message(value, author, scope, false)?)
        }
        model::BodyDto::AnswerGiven(value) => Output::AnswerGiven {
            thread_id: domain::ThreadId::from_bytes(value.thread.0),
            message: message(&value.message, author, scope, false)?,
        },
        model::BodyDto::ThreadCancelled(value) => Output::ThreadCancelled {
            thread_id: domain::ThreadId::from_bytes(value.thread.0),
            reason: optional_content(&value.reason)?,
        },
        model::BodyDto::MessageArchived(value) => Output::MessageArchived {
            message_id: domain::MessageId::from_bytes(value.message.0),
        },
        model::BodyDto::MessageRestored(value) => Output::MessageRestored {
            message_id: domain::MessageId::from_bytes(value.message.0),
        },
        model::BodyDto::MessageRejected(value) => Output::MessageRejected {
            message_id: domain::MessageId::from_bytes(value.message.0),
            reason: error_code(&value.reason)?,
        },
        model::BodyDto::HarnessActivityRecorded(value) => {
            ensure(
                value.source.installation.0 == *author.installation_id().as_bytes(),
                FailureClass::AuthorSubjectMismatch,
            )?;
            ensure(
                match scope {
                    domain::FactScope::InstallationPrivate(scope_installation) => {
                        *scope_installation == installation(value.source.installation)
                    }
                    domain::FactScope::AccountAddressed(_) => true,
                    domain::FactScope::PeerAddressed(_)
                    | domain::FactScope::RemoteControl { .. } => false,
                },
                FailureClass::ScopePayloadMismatch,
            )?;
            Output::HarnessActivityRecorded {
                project: value
                    .project
                    .as_ref()
                    .map(|project| {
                        Ok(domain::ProjectActivityAttribution {
                            project_id: domain::ProjectId::from_bytes(project.project.0),
                            dispatch_id: domain::DispatchId::from_bytes(project.dispatch.0),
                            binding: binding(&project.binding)?,
                            thread_id: domain::ThreadId::from_bytes(project.thread.0),
                        })
                    })
                    .transpose()?,
                source: mailbox_address(&value.source),
                correlation: operation(&value.operation)?,
                item: optional_short(&value.item)?,
                kind: activity_kind(value.kind),
                logical_key: short(&value.logical_key)?,
                runtime: short(&value.runtime)?,
                sequence: value.sequence,
                occurred_at: timestamp(value.occurred_at)?,
                status: activity_status(&value.status)?,
                content: content(&value.content)?,
                truncated: value.truncated,
            }
        }
        model::BodyDto::AgentNameClaimed(value) => Output::AgentNameClaimed {
            agent_id: domain::AgentId::from_bytes(value.agent.0),
            mailbox_id: mailbox(value.mailbox),
            name: short(&value.name)?,
        },
        model::BodyDto::AgentRetired(value) => Output::AgentRetired {
            agent_id: domain::AgentId::from_bytes(value.agent.0),
            mailbox_id: mailbox(value.mailbox),
        },
        model::BodyDto::ProviderSessionSelected(value) => Output::ProviderSessionSelected {
            agent_id: domain::AgentId::from_bytes(value.agent.0),
            mailbox_id: mailbox(value.mailbox),
            provider: provider(&value.provider)?,
            session: session(&value.session)?,
            context: context(&value.context)?,
        },
        model::BodyDto::ProviderSessionRenamed(value) => Output::ProviderSessionRenamed {
            agent_id: domain::AgentId::from_bytes(value.agent.0),
            provider: provider(&value.provider)?,
            session: session(&value.session)?,
            display_name: optional_short(&value.display)?,
        },
        model::BodyDto::ProjectCreated(value) => {
            ensure(
                installation(value.home) == author.installation_id(),
                FailureClass::AuthorSubjectMismatch,
            )?;
            Output::ProjectCreated {
                project_id: project(value.project),
                mailbox_id: mailbox(value.mailbox),
                home: installation(value.home),
                name: short(&value.name)?,
                brief: optional_content(&value.brief)?,
                predecessor: value.predecessor.0.map(project),
                resources: domain::BoundedVec::new(
                    value
                        .resources
                        .iter()
                        .map(resource)
                        .collect::<Result<Vec<_>, _>>()?,
                )
                .map_err(domain_value)?,
                primary: value
                    .primary
                    .0
                    .map(|id| domain::ResourceId::from_bytes(id.0)),
                initial_state: initial_state(value.state),
            }
        }
        model::BodyDto::ProjectOpened(value) => Output::ProjectOpened {
            project_id: project(value.project),
        },
        model::BodyDto::ProjectClosingStarted(value) => Output::ProjectClosingStarted {
            project_id: project(value.project),
        },
        model::BodyDto::ProjectClosed(value) => Output::ProjectClosed {
            project_id: project(value.project),
            forced: value.forced,
            runtime: optional_runtime(&value.runtime)?,
        },
        model::BodyDto::ProjectArchived(value) => Output::ProjectArchived {
            project_id: project(value.project),
        },
        model::BodyDto::ProjectUnarchived(value) => Output::ProjectUnarchived {
            project_id: project(value.project),
        },
        model::BodyDto::ProjectMetadataUpdated(value) => Output::ProjectMetadataUpdated {
            project_id: project(value.project),
            name: short(&value.name)?,
            brief: optional_content(&value.brief)?,
        },
        model::BodyDto::ProjectResourceAdded(value) => Output::ProjectResourceAdded {
            project_id: project(value.project),
            resource: resource(&value.resource)?,
            make_primary: value.primary,
        },
        model::BodyDto::ProjectResourceRemoved(value) => Output::ProjectResourceRemoved {
            project_id: project(value.project),
            resource_id: domain::ResourceId::from_bytes(value.resource.0),
            force: value.force,
        },
        model::BodyDto::ProjectResourceReplaced(value) => Output::ProjectResourceReplaced {
            project_id: project(value.project),
            old_resource_id: domain::ResourceId::from_bytes(value.old_resource.0),
            new_resource: resource(&value.resource)?,
        },
        model::BodyDto::ProjectPrimaryResourceChanged(value) => {
            Output::ProjectPrimaryResourceChanged {
                project_id: project(value.project),
                resource_id: domain::ResourceId::from_bytes(value.resource.0),
            }
        }
        model::BodyDto::ProjectResourceHealthObserved(value) => {
            Output::ProjectResourceHealthObserved {
                project_id: project(value.project),
                resource_id: domain::ResourceId::from_bytes(value.resource.0),
                health: resource_health(value.health),
                details: optional_content(&value.details)?,
                checked_at: timestamp(value.checked_at)?,
            }
        }
        model::BodyDto::ProjectAssignmentConfiguring(value) => {
            Output::ProjectAssignmentConfiguring {
                project_id: project(value.project),
                intent: domain::AssignmentIntent {
                    assignment_id: domain::AssignmentId::from_bytes(value.assignment.0),
                    agent_id: domain::AgentId::from_bytes(value.agent.0),
                    provider: provider(&value.provider)?,
                },
            }
        }
        model::BodyDto::ProjectAssignmentRunnable(value) => Output::ProjectAssignmentRunnable {
            project_id: project(value.project),
            binding: binding(&value.binding)?,
            thread_id: domain::ThreadId::from_bytes(value.thread.0),
            launch_directory: locator(&value.launch_directory)?,
            activation: operation(&value.activation)?,
        },
        model::BodyDto::ProjectAssignmentBlocked(value) => Output::ProjectAssignmentBlocked {
            project_id: project(value.project),
            assignment_id: domain::AssignmentId::from_bytes(value.assignment.0),
            cause: error_code(&value.cause)?,
        },
        model::BodyDto::ProjectAssignmentEnded(value) => Output::ProjectAssignmentEnded {
            project_id: project(value.project),
            assignment_id: domain::AssignmentId::from_bytes(value.assignment.0),
            forced: value.forced,
            runtime: optional_runtime(&value.runtime)?,
        },
        model::BodyDto::ProjectInputAccepted(value) => Output::ProjectInputAccepted {
            project_id: project(value.project),
            message_id: domain::MessageId::from_bytes(value.message.0),
            input_fact_id: domain::FactId::from_bytes(value.input_fact.0),
            sequence: value.sequence,
        },
        model::BodyDto::ProjectInputDispatched(value) => Output::ProjectInputDispatched {
            project_id: project(value.project),
            message_id: domain::MessageId::from_bytes(value.message.0),
            sequence: value.sequence,
            dispatch_id: domain::DispatchId::from_bytes(value.dispatch.0),
            binding: binding(&value.binding)?,
            thread_id: domain::ThreadId::from_bytes(value.thread.0),
        },
        model::BodyDto::ProjectOutputRecorded(value) => {
            ensure(
                value.output == value.message.id,
                FailureClass::PayloadInvariant,
            )?;
            Output::ProjectOutputRecorded {
                project_id: project(value.project),
                output_id: domain::MessageId::from_bytes(value.output.0),
                dispatch_id: domain::DispatchId::from_bytes(value.dispatch.0),
                binding: binding(&value.binding)?,
                thread_id: domain::ThreadId::from_bytes(value.thread.0),
                message: message(&value.message, author, scope, true)?,
            }
        }
        model::BodyDto::RemoteProjectCommandRequested(value) => {
            Output::RemoteProjectCommandRequested {
                command_id: domain::CommandId::from_bytes(value.command.0),
                digest: domain::CommandDigest::from_bytes(value.digest.0),
                project_id: project(value.project),
                target_home: installation(value.target_home),
                expected_head: value
                    .expected_head
                    .0
                    .map(|head| domain::FactId::from_bytes(head.0)),
                operation: operation(&value.operation)?,
                body: content(&value.body)?,
            }
        }
        model::BodyDto::RemoteProjectCommandReceipt(value) => Output::RemoteProjectCommandReceipt {
            command_id: domain::CommandId::from_bytes(value.command.0),
            digest: domain::CommandDigest::from_bytes(value.digest.0),
            project_id: project(value.project),
            received_head: value
                .received_head
                .0
                .map(|head| domain::FactId::from_bytes(head.0)),
            received_at: timestamp(value.received_at)?,
        },
        model::BodyDto::RemoteProjectCommandOutcome(value) => Output::RemoteProjectCommandOutcome {
            command_id: domain::CommandId::from_bytes(value.command.0),
            digest: domain::CommandDigest::from_bytes(value.digest.0),
            project_id: project(value.project),
            result: remote_result(&value.result)?,
            runtime: optional_runtime(&value.runtime)?,
        },
    })
}

fn message(
    value: &model::MessageDto,
    author: domain::InstallationAddress,
    scope: &domain::FactScope,
    project_output: bool,
) -> Result<domain::MessageContent, ProtocolError> {
    let sender = mailbox_address(&value.sender);
    let recipient = value.recipient.0.as_ref().map(mailbox_address);
    ensure(
        sender.installation_id() == author.installation_id(),
        FailureClass::AuthorSubjectMismatch,
    )?;
    let routing_matches = match scope {
        domain::FactScope::InstallationPrivate(installation) => {
            *installation == sender.installation_id()
                && recipient.is_some_and(|target| target.installation_id() == *installation)
        }
        domain::FactScope::PeerAddressed(mailbox) => recipient == Some(*mailbox),
        domain::FactScope::AccountAddressed(_) => {
            recipient.is_some() == (project_output || value.project.0.is_some())
        }
        domain::FactScope::RemoteControl { .. } => false,
    };
    ensure(routing_matches, FailureClass::ScopePayloadMismatch)?;
    Ok(domain::MessageContent {
        message_id: domain::MessageId::from_bytes(value.id.0),
        sender,
        recipient,
        body: content(&value.body)?,
        purpose: message_purpose(value.purpose),
        presentation: presentation(value.presentation),
        correlation: value.correlation.0.as_ref().map(operation).transpose()?,
        project_id: value.project.0.map(project),
    })
}

fn context(value: &model::ContextDto) -> Result<domain::RepositoryContext, ProtocolError> {
    Ok(domain::RepositoryContext {
        directory: locator(&value.directory)?,
        repository: value.repository.0.as_ref().map(locator).transpose()?,
        worktree: value.worktree.0.as_ref().map(locator).transpose()?,
        branch: optional_short(&value.branch)?,
    })
}

fn locator(value: &model::LocatorDto) -> Result<domain::ResourceLocator, ProtocolError> {
    let scheme = match value.scheme {
        model::LocatorSchemeDto::Git => domain::ResourceScheme::GitRepository,
        model::LocatorSchemeDto::Worktree => domain::ResourceScheme::WorkingTree,
        model::LocatorSchemeDto::Container => domain::ResourceScheme::Container,
        model::LocatorSchemeDto::Opaque => domain::ResourceScheme::Opaque,
    };
    let text = domain::BoundedText::new(value.value.0.clone()).map_err(domain_value)?;
    Ok(domain::ResourceLocator::new(scheme, text))
}

fn operation(value: &model::OperationDto) -> Result<domain::OperationCorrelation, ProtocolError> {
    Ok(domain::OperationCorrelation::new(
        provider(&value.provider)?,
        session(&value.session)?,
        domain::OperationId::from_bytes(value.id.0),
    ))
}

fn resource(value: &model::ResourceDto) -> Result<domain::ProjectResource, ProtocolError> {
    Ok(domain::ProjectResource {
        resource_id: domain::ResourceId::from_bytes(value.id.0),
        display_locator: locator(&value.display)?,
        canonical_locator: locator(&value.canonical)?,
        health: resource_health(value.health),
    })
}

fn binding(value: &model::BindingDto) -> Result<domain::AssignmentBinding, ProtocolError> {
    Ok(domain::AssignmentBinding {
        assignment_id: domain::AssignmentId::from_bytes(value.assignment.0),
        agent_id: domain::AgentId::from_bytes(value.agent.0),
        provider: provider(&value.provider)?,
        session: session(&value.session)?,
    })
}

fn activity_status(
    value: &model::ActivityStatusDto,
) -> Result<domain::ActivityStatus, ProtocolError> {
    Ok(match value {
        model::ActivityStatusDto::Simple(status) => match status.state {
            model::ActivitySimpleStateDto::Snapshot => domain::ActivityStatus::Snapshot,
            model::ActivitySimpleStateDto::Running => domain::ActivityStatus::Running,
            model::ActivitySimpleStateDto::Succeeded => domain::ActivityStatus::Succeeded,
            model::ActivitySimpleStateDto::Interrupted => domain::ActivityStatus::Interrupted,
        },
        model::ActivityStatusDto::Failed(status) => {
            domain::ActivityStatus::Failed(error_code(&status.code)?)
        }
    })
}

fn optional_runtime(
    value: &model::RequiredOption<model::RuntimeDto>,
) -> Result<Option<domain::RuntimeObservation>, ProtocolError> {
    value.0.as_ref().map(runtime).transpose()
}

fn runtime(value: &model::RuntimeDto) -> Result<domain::RuntimeObservation, ProtocolError> {
    Ok(match value {
        model::RuntimeDto::Succeeded(_) => domain::RuntimeObservation::Succeeded,
        model::RuntimeDto::Failed(status) => {
            domain::RuntimeObservation::Failed(error_code(&status.code)?)
        }
        model::RuntimeDto::Uncertain(status) => {
            domain::RuntimeObservation::Uncertain(error_code(&status.code)?)
        }
    })
}

fn remote_result(
    value: &model::RemoteResultDto,
) -> Result<domain::RemoteCommandResult, ProtocolError> {
    Ok(match value {
        model::RemoteResultDto::Committed(result) => {
            domain::RemoteCommandResult::Committed(domain::FactId::from_bytes(result.head.0))
        }
        model::RemoteResultDto::Rejected(result) => domain::RemoteCommandResult::Rejected {
            error: error_code(&result.code)?,
            external_state_warning: result
                .external_state_warning
                .0
                .as_ref()
                .map(|warning| match warning.kind {
                    model::ExternalStateWarningKindDto::WorktreeMayExist => {
                        Ok(domain::ProjectExternalStateWarning::WorktreeMayExist {
                            destination: locator(&warning.destination)?,
                            branch: domain::ShortText::new(warning.branch.0.clone())
                                .map_err(domain_value)?,
                        })
                    }
                })
                .transpose()?,
        },
    })
}

const fn mailbox_kind(value: model::MailboxKindDto) -> domain::MailboxKind {
    match value {
        model::MailboxKindDto::Human => domain::MailboxKind::Human,
        model::MailboxKindDto::Agent => domain::MailboxKind::Agent,
    }
}

const fn message_purpose(value: model::MessagePurposeDto) -> domain::MessagePurpose {
    match value {
        model::MessagePurposeDto::Question => domain::MessagePurpose::Question,
        model::MessagePurposeDto::Asynchronous => domain::MessagePurpose::Asynchronous,
        model::MessagePurposeDto::ProjectOutput => domain::MessagePurpose::ProjectOutput,
    }
}

const fn presentation(value: model::PresentationDto) -> domain::PresentationKind {
    match value {
        model::PresentationDto::Message => domain::PresentationKind::Message,
        model::PresentationDto::FinalAnswer => domain::PresentationKind::FinalAnswer,
        model::PresentationDto::Status => domain::PresentationKind::Status,
    }
}

const fn activity_kind(value: model::ActivityKindDto) -> domain::ActivityKind {
    match value {
        model::ActivityKindDto::Status => domain::ActivityKind::Status,
        model::ActivityKindDto::Progress => domain::ActivityKind::Progress,
        model::ActivityKindDto::Plan => domain::ActivityKind::Plan,
        model::ActivityKindDto::Diff => domain::ActivityKind::Diff,
        model::ActivityKindDto::CompletedItem => domain::ActivityKind::CompletedItem,
    }
}

const fn initial_state(value: model::InitialStateDto) -> domain::InitialProjectState {
    match value {
        model::InitialStateDto::Open => domain::InitialProjectState::Open,
        model::InitialStateDto::Closed => domain::InitialProjectState::Closed,
    }
}

const fn resource_health(value: model::ResourceHealthDto) -> domain::ResourceHealth {
    match value {
        model::ResourceHealthDto::Unknown => domain::ResourceHealth::Unknown,
        model::ResourceHealthDto::Healthy => domain::ResourceHealth::Healthy,
        model::ResourceHealthDto::Degraded => domain::ResourceHealth::Degraded,
        model::ResourceHealthDto::Unavailable => domain::ResourceHealth::Unavailable,
    }
}

fn require_mailbox_scope(
    scope: &domain::FactScope,
    value: &model::MailboxAddressDto,
) -> Result<(), ProtocolError> {
    ensure(
        matches!(scope, domain::FactScope::PeerAddressed(mailbox) if *mailbox == mailbox_address(value)),
        FailureClass::ScopePayloadMismatch,
    )
}

fn require_account_scope(
    scope: &domain::FactScope,
    value: model::Hex32,
) -> Result<(), ProtocolError> {
    ensure(
        matches!(scope, domain::FactScope::AccountAddressed(account_id) if *account_id == account(value)),
        FailureClass::ScopePayloadMismatch,
    )
}

const fn installation(value: model::Hex32) -> domain::InstallationId {
    domain::InstallationId::from_bytes(value.0)
}

const fn mailbox(value: model::Hex32) -> domain::MailboxId {
    domain::MailboxId::from_bytes(value.0)
}

const fn account(value: model::Hex32) -> domain::AccountId {
    domain::AccountId::from_bytes(value.0)
}

const fn project(value: model::Hex32) -> domain::ProjectId {
    domain::ProjectId::from_bytes(value.0)
}

const fn signing(value: model::Hex32) -> domain::SigningPublicKey {
    domain::SigningPublicKey::from_bytes(value.0)
}

const fn installation_address(
    value: &model::InstallationAddressDto,
) -> domain::InstallationAddress {
    domain::InstallationAddress::new(installation(value.installation), signing(value.signing))
}

const fn mailbox_address(value: &model::MailboxAddressDto) -> domain::MailboxAddress {
    domain::MailboxAddress::new(installation(value.installation), mailbox(value.mailbox))
}

fn timestamp(value: model::Milliseconds) -> Result<domain::Timestamp, ProtocolError> {
    let value = i64::try_from(value.0).map_err(|_| failure(FailureClass::DomainValueInvalid))?;
    Ok(domain::Timestamp::from_unix_millis(value))
}

fn short(value: &model::ShortText) -> Result<domain::ShortText, ProtocolError> {
    domain::ShortText::new(value.0.clone()).map_err(domain_value)
}

fn optional_short(
    value: &model::RequiredOption<model::ShortText>,
) -> Result<Option<domain::ShortText>, ProtocolError> {
    value.0.as_ref().map(short).transpose()
}

fn content(value: &model::ContentText) -> Result<domain::ContentText, ProtocolError> {
    domain::ContentText::new(value.0.clone()).map_err(domain_value)
}

fn optional_content(
    value: &model::RequiredOption<model::ContentText>,
) -> Result<Option<domain::ContentText>, ProtocolError> {
    value.0.as_ref().map(content).transpose()
}

fn provider(value: &model::ProviderText) -> Result<domain::ProviderId, ProtocolError> {
    domain::ProviderId::new(value.0.clone()).map_err(domain_value)
}

fn session(value: &model::SessionText) -> Result<domain::ProviderSessionId, ProtocolError> {
    domain::ProviderSessionId::new(value.0.clone()).map_err(domain_value)
}

fn error_code(value: &model::ShortText) -> Result<domain::ErrorCode, ProtocolError> {
    domain::ErrorCode::new(value.0.clone()).map_err(domain_value)
}

const fn failure(class: FailureClass) -> ProtocolError {
    ProtocolError::new(class)
}

fn domain_value(_: domain::ValidatedValueError) -> ProtocolError {
    failure(FailureClass::DomainValueInvalid)
}

fn ensure(condition: bool, class: FailureClass) -> Result<(), ProtocolError> {
    if condition {
        Ok(())
    } else {
        Err(failure(class))
    }
}
