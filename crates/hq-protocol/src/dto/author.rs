//! Typed semantic planning into the private canonical v1 DTO model.

use hq_domain as domain;

use super::{decode::decode_unsigned_content, encode_dto, model, semantic};
use crate::{Bip340Signer, DispatchOutcome, FailureClass, ProtocolError};

/// Deterministic unsigned semantic event inputs owned independently of protocol DTOs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalEventPlan {
    author: domain::InstallationId,
    authored_at: domain::Timestamp,
    scope: domain::FactScope,
    causal:
        domain::CausalReferences<{ domain::MAX_FACT_PARENTS }, { domain::MAX_FACT_AUTHORITIES }>,
    payload: domain::SemanticPayload,
}

impl CanonicalEventPlan {
    /// Creates a typed event plan from explicit deterministic inputs.
    pub const fn new(
        author: domain::InstallationId,
        authored_at: domain::Timestamp,
        scope: domain::FactScope,
        causal: domain::CausalReferences<
            { domain::MAX_FACT_PARENTS },
            { domain::MAX_FACT_AUTHORITIES },
        >,
        payload: domain::SemanticPayload,
    ) -> Self {
        Self {
            author,
            authored_at,
            scope,
            causal,
            payload,
        }
    }

    /// Copies the semantic inputs of an existing fact, excluding its content-derived identity.
    pub fn from_fact(fact: &domain::SemanticFact) -> Self {
        Self::new(
            fact.author().installation_id(),
            fact.authored_at(),
            fact.scope().clone(),
            fact.causal().clone(),
            fact.payload().clone(),
        )
    }

    /// Encodes deterministic unsigned semantic content for the local planning boundary.
    ///
    /// These bytes are not a fact and carry no authorship evidence. They become admissible only
    /// after [`Self::sign`] produces and verifies an ordinary signed canonical event.
    pub fn encode_content(self) -> Result<Vec<u8>, ProtocolError> {
        let millis = u64::try_from(self.authored_at.as_unix_millis())
            .map_err(|_| ProtocolError::new(FailureClass::DomainValueInvalid))?;
        encode_dto(&self.into_dto(millis))
    }

    /// Strictly decodes untrusted canonical unsigned content from the local planning boundary.
    ///
    /// The result still has no domain authority. Signing and ordinary verification remain required
    /// before the plan can enter the canonical fact set.
    pub fn decode_content(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let dto = decode_unsigned_content(bytes)?;
        if encode_dto(&dto)?.as_slice() != bytes {
            return Err(ProtocolError::new(FailureClass::ContentNonCanonical));
        }
        semantic::plan(&dto)
    }

    /// Consumes the plan into its deterministic semantic authoring inputs.
    pub fn into_parts(
        self,
    ) -> (
        domain::InstallationId,
        domain::Timestamp,
        domain::FactScope,
        domain::CausalReferences<{ domain::MAX_FACT_PARENTS }, { domain::MAX_FACT_AUTHORITIES }>,
        domain::SemanticPayload,
    ) {
        (
            self.author,
            self.authored_at,
            self.scope,
            self.causal,
            self.payload,
        )
    }

    /// Canonically encodes, signs, and reruns every ordinary verification transition.
    pub fn sign(
        self,
        signer: &Bip340Signer,
        auxiliary_randomness: [u8; 32],
    ) -> Result<super::VerifiedSemanticFact, ProtocolError> {
        let millis = u64::try_from(self.authored_at.as_unix_millis())
            .map_err(|_| ProtocolError::new(FailureClass::DomainValueInvalid))?;
        let dto = self.into_dto(millis);
        let content = encode_dto(&dto)?;
        let event = signer.sign(millis / 1_000, &content, auxiliary_randomness)?;
        let DispatchOutcome::Supported(supported) = event.dispatch()? else {
            return Err(ProtocolError::new(FailureClass::NamespaceConfusion));
        };
        supported.decode_v1()?.into_semantic_fact()
    }

    fn into_dto(self, millis: u64) -> model::ContentDto {
        let protocol = match self.payload.kind().protocol_class() {
            domain::ProtocolClass::Canonical => model::ProtocolDto::Canonical,
            domain::ProtocolClass::RemoteControl => model::ProtocolDto::Control,
        };
        let body = body(&self.payload);
        let namespace = |fact_id: domain::FactId| {
            if protocol == model::ProtocolDto::Control
                && self.causal.authority(domain::AuthorityRole::Request) == Some(fact_id)
            {
                model::NamespaceDto::Control
            } else {
                model::NamespaceDto::Canonical
            }
        };
        let mut parents = self
            .causal
            .parents()
            .iter()
            .map(|fact_id| model::ParentDto(namespace(*fact_id), id(fact_id)))
            .collect::<Vec<_>>();
        parents.sort_unstable();
        let mut authorities = domain::AuthorityRole::ALL
            .into_iter()
            .filter_map(|role| {
                self.causal.authority(role).map(|fact_id| {
                    model::AuthorityDto(role_dto(role), namespace(fact_id), id(&fact_id))
                })
            })
            .collect::<Vec<_>>();
        authorities.sort_unstable();
        model::ContentDto {
            protocol,
            version: 1,
            family: body.family(),
            author: id(&self.author),
            time: model::Milliseconds(millis),
            scope: scope(&self.scope),
            parents,
            authorities,
            body,
        }
    }
}

fn scope(value: &domain::FactScope) -> model::ScopeDto {
    match value {
        domain::FactScope::InstallationPrivate(installation) => {
            model::ScopeDto::Local((model::LocalTag::Local, id(installation)))
        }
        domain::FactScope::PeerAddressed(mailbox) => model::ScopeDto::Peer((
            model::PeerTag::Peer,
            id(&mailbox.installation_id()),
            id(&mailbox.mailbox_id()),
        )),
        domain::FactScope::AccountAddressed(account) => {
            model::ScopeDto::Account((model::AccountTag::Account, id(account)))
        }
        domain::FactScope::RemoteControl {
            account_id,
            target_home,
        } => {
            model::ScopeDto::Control((model::ControlTag::Control, id(account_id), id(target_home)))
        }
    }
}

#[allow(clippy::too_many_lines)]
fn body(value: &domain::SemanticPayload) -> model::BodyDto {
    use domain::SemanticPayload as Input;
    use model::BodyDto as Output;

    match value {
        Input::InstallationDeclared {
            installation_id,
            signing_key,
            encryption_key,
            label,
        } => Output::InstallationDeclared(model::InstallationDeclaredDto {
            installation: id(installation_id),
            signing: id(signing_key),
            encryption: id(encryption_key),
            label: optional_short(label.as_ref()),
        }),
        Input::MailboxCreated {
            mailbox_id,
            kind,
            label,
        } => Output::MailboxCreated(model::MailboxCreatedDto {
            mailbox: id(mailbox_id),
            kind: mailbox_kind(*kind),
            label: optional_short(label.as_ref()),
        }),
        Input::MailboxSessionBound {
            mailbox_id,
            provider,
            session,
        } => Output::MailboxSessionBound(model::MailboxSessionBoundDto {
            mailbox: id(mailbox_id),
            provider: provider_text(provider),
            session: session_text(session),
        }),
        Input::MailboxContextRecorded {
            mailbox_id,
            context,
        } => Output::MailboxContextRecorded(model::MailboxContextRecordedDto {
            mailbox: id(mailbox_id),
            context: context_dto(context),
        }),
        Input::PeerRouteSet {
            peer,
            encryption_key,
            label,
            relay_hints,
        } => Output::PeerRouteSet(model::PeerRouteSetDto {
            peer: installation_address(*peer),
            encryption: id(encryption_key),
            label: optional_short(label.as_ref()),
            relays: relay_hints.as_slice().iter().map(locator).collect(),
        }),
        Input::PeerRouteBlocked { peer_id, reason } => {
            Output::PeerRouteBlocked(model::PeerRouteBlockedDto {
                peer: id(peer_id),
                reason: short_string(reason.as_str()),
            })
        }
        Input::MailboxAccessGranted {
            grant_id,
            mailbox,
            grantee,
        } => Output::MailboxAccessGranted(model::MailboxAccessGrantedDto {
            grant: id(grant_id),
            mailbox: mailbox_address(*mailbox),
            grantee: installation_address(*grantee),
        }),
        Input::MailboxAccessRevoked {
            grant_id,
            mailbox,
            grantee_id,
        } => Output::MailboxAccessRevoked(model::MailboxAccessRevokedDto {
            grant: id(grant_id),
            mailbox: mailbox_address(*mailbox),
            grantee: id(grantee_id),
        }),
        Input::MailboxActionObserved {
            grant_id,
            action_id,
        } => Output::MailboxActionObserved(model::MailboxActionObservedDto {
            grant: id(grant_id),
            action: id(action_id),
        }),
        Input::HumanAccountCreated {
            account_id,
            creator,
            label,
        } => Output::HumanAccountCreated(model::HumanAccountCreatedDto {
            account: id(account_id),
            creator: installation_address(*creator),
            label: optional_short(label.as_ref()),
        }),
        Input::HumanAccountSelected { account_id } => {
            Output::HumanAccountSelected(model::HumanAccountSelectedDto {
                account: id(account_id),
            })
        }
        Input::HumanDeviceGranted {
            account_id,
            grant_id,
            device,
            label,
            relay_hints,
        } => Output::HumanDeviceGranted(model::HumanDeviceGrantedDto {
            account: id(account_id),
            grant: id(grant_id),
            device: installation_address(*device),
            label: optional_short(label.as_ref()),
            relays: relay_hints.as_slice().iter().map(locator).collect(),
        }),
        Input::HumanDeviceAccepted {
            account_id,
            grant_id,
            device,
        } => Output::HumanDeviceAccepted(model::HumanDeviceAcceptedDto {
            account: id(account_id),
            grant: id(grant_id),
            device: installation_address(*device),
        }),
        Input::HumanDeviceRevoked {
            account_id,
            grant_id,
            device_id,
        } => Output::HumanDeviceRevoked(model::HumanDeviceRevokedDto {
            account: id(account_id),
            grant: id(grant_id),
            device: id(device_id),
        }),
        Input::QuestionAsked(message) => Output::QuestionAsked(message_dto(message)),
        Input::AsynchronousMessageSent { thread_id, message } => {
            Output::AsynchronousMessageSent(model::AsynchronousMessageSentDto {
                thread: model::RequiredOption(thread_id.as_ref().map(id)),
                message: message_dto(message),
            })
        }
        Input::AnswerGiven { thread_id, message } => Output::AnswerGiven(model::AnswerGivenDto {
            thread: id(thread_id),
            message: message_dto(message),
        }),
        Input::ThreadCancelled { thread_id, reason } => {
            Output::ThreadCancelled(model::ThreadCancelledDto {
                thread: id(thread_id),
                reason: optional_content(reason.as_ref()),
            })
        }
        Input::MessageArchived { message_id } => Output::MessageArchived(model::MessageTargetDto {
            message: id(message_id),
        }),
        Input::MessageRestored { message_id } => Output::MessageRestored(model::MessageTargetDto {
            message: id(message_id),
        }),
        Input::MessageRejected { message_id, reason } => {
            Output::MessageRejected(model::MessageRejectedDto {
                message: id(message_id),
                reason: short_string(reason.as_str()),
            })
        }
        Input::HarnessActivityRecorded {
            project,
            source,
            correlation,
            item,
            kind,
            logical_key,
            runtime,
            sequence,
            occurred_at,
            status,
            content,
            truncated,
        } => Output::HarnessActivityRecorded(model::HarnessActivityRecordedDto {
            project: project
                .as_ref()
                .map(|project| model::ProjectActivityAttributionDto {
                    project: id(&project.project_id),
                    dispatch: id(&project.dispatch_id),
                    binding: binding(&project.binding),
                    thread: id(&project.thread_id),
                }),
            source: mailbox_address(*source),
            operation: operation(correlation),
            item: optional_short(item.as_ref()),
            kind: activity_kind(*kind),
            logical_key: short(logical_key),
            runtime: short(runtime),
            sequence: *sequence,
            occurred_at: milliseconds(*occurred_at),
            status: activity_status(status),
            content: content_text(content),
            truncated: *truncated,
        }),
        Input::AgentNameClaimed {
            agent_id,
            mailbox_id,
            name,
        } => Output::AgentNameClaimed(model::AgentNameClaimedDto {
            agent: id(agent_id),
            mailbox: id(mailbox_id),
            name: short(name),
        }),
        Input::AgentRetired {
            agent_id,
            mailbox_id,
        } => Output::AgentRetired(model::AgentRetiredDto {
            agent: id(agent_id),
            mailbox: id(mailbox_id),
        }),
        Input::ProviderSessionSelected {
            agent_id,
            mailbox_id,
            provider,
            session,
            context,
        } => Output::ProviderSessionSelected(model::ProviderSessionSelectedDto {
            agent: id(agent_id),
            mailbox: id(mailbox_id),
            provider: provider_text(provider),
            session: session_text(session),
            context: context_dto(context),
        }),
        Input::ProviderSessionRenamed {
            agent_id,
            provider,
            session,
            display_name,
        } => Output::ProviderSessionRenamed(model::ProviderSessionRenamedDto {
            agent: id(agent_id),
            provider: provider_text(provider),
            session: session_text(session),
            display: optional_short(display_name.as_ref()),
        }),
        Input::ProjectCreated {
            project_id,
            mailbox_id,
            home,
            name,
            brief,
            predecessor,
            resources,
            primary,
            initial_state,
        } => Output::ProjectCreated(model::ProjectCreatedDto {
            project: id(project_id),
            mailbox: id(mailbox_id),
            home: id(home),
            name: short(name),
            brief: optional_content(brief.as_ref()),
            predecessor: model::RequiredOption(predecessor.as_ref().map(id)),
            resources: resources.as_slice().iter().map(resource).collect(),
            primary: model::RequiredOption(primary.as_ref().map(id)),
            state: initial_state_dto(*initial_state),
        }),
        Input::ProjectOpened { project_id } => Output::ProjectOpened(project_target(project_id)),
        Input::ProjectClosingStarted { project_id } => {
            Output::ProjectClosingStarted(project_target(project_id))
        }
        Input::ProjectClosed {
            project_id,
            forced,
            runtime,
        } => Output::ProjectClosed(model::ProjectClosedDto {
            project: id(project_id),
            forced: *forced,
            runtime: optional_runtime(runtime.as_ref()),
        }),
        Input::ProjectArchived { project_id } => {
            Output::ProjectArchived(project_target(project_id))
        }
        Input::ProjectUnarchived { project_id } => {
            Output::ProjectUnarchived(project_target(project_id))
        }
        Input::ProjectMetadataUpdated {
            project_id,
            name,
            brief,
        } => Output::ProjectMetadataUpdated(model::ProjectMetadataUpdatedDto {
            project: id(project_id),
            name: short(name),
            brief: optional_content(brief.as_ref()),
        }),
        Input::ProjectResourceAdded {
            project_id,
            resource: value,
            make_primary,
        } => Output::ProjectResourceAdded(model::ProjectResourceAddedDto {
            project: id(project_id),
            resource: resource(value),
            primary: *make_primary,
        }),
        Input::ProjectResourceRemoved {
            project_id,
            resource_id,
            force,
        } => Output::ProjectResourceRemoved(model::ProjectResourceRemovedDto {
            project: id(project_id),
            resource: id(resource_id),
            force: *force,
        }),
        Input::ProjectResourceReplaced {
            project_id,
            old_resource_id,
            new_resource,
        } => Output::ProjectResourceReplaced(model::ProjectResourceReplacedDto {
            project: id(project_id),
            old_resource: id(old_resource_id),
            resource: resource(new_resource),
        }),
        Input::ProjectPrimaryResourceChanged {
            project_id,
            resource_id,
        } => Output::ProjectPrimaryResourceChanged(model::ProjectResourceTargetDto {
            project: id(project_id),
            resource: id(resource_id),
        }),
        Input::ProjectResourceHealthObserved {
            project_id,
            resource_id,
            health,
            details,
            checked_at,
        } => Output::ProjectResourceHealthObserved(model::ProjectResourceHealthObservedDto {
            project: id(project_id),
            resource: id(resource_id),
            health: resource_health(*health),
            details: optional_content(details.as_ref()),
            checked_at: milliseconds(*checked_at),
        }),
        Input::ProjectAssignmentConfiguring { project_id, intent } => {
            Output::ProjectAssignmentConfiguring(model::ProjectAssignmentConfiguringDto {
                project: id(project_id),
                assignment: id(&intent.assignment_id),
                agent: id(&intent.agent_id),
                provider: provider_text(&intent.provider),
            })
        }
        Input::ProjectAssignmentRunnable {
            project_id,
            binding: value,
            thread_id,
            launch_directory,
            activation,
        } => Output::ProjectAssignmentRunnable(model::ProjectAssignmentRunnableDto {
            project: id(project_id),
            binding: binding(value),
            thread: id(thread_id),
            launch_directory: locator(launch_directory),
            activation: operation(activation),
        }),
        Input::ProjectAssignmentBlocked {
            project_id,
            assignment_id,
            cause,
        } => Output::ProjectAssignmentBlocked(model::ProjectAssignmentBlockedDto {
            project: id(project_id),
            assignment: id(assignment_id),
            cause: short_string(cause.as_str()),
        }),
        Input::ProjectAssignmentEnded {
            project_id,
            assignment_id,
            forced,
            runtime,
        } => Output::ProjectAssignmentEnded(model::ProjectAssignmentEndedDto {
            project: id(project_id),
            assignment: id(assignment_id),
            forced: *forced,
            runtime: optional_runtime(runtime.as_ref()),
        }),
        Input::ProjectInputAccepted {
            project_id,
            message_id,
            input_fact_id,
            sequence,
        } => Output::ProjectInputAccepted(model::ProjectInputAcceptedDto {
            project: id(project_id),
            message: id(message_id),
            input_fact: id(input_fact_id),
            sequence: *sequence,
        }),
        Input::ProjectInputDispatched {
            project_id,
            message_id,
            sequence,
            dispatch_id,
            binding: value,
            thread_id,
        } => Output::ProjectInputDispatched(model::ProjectInputDispatchedDto {
            project: id(project_id),
            message: id(message_id),
            sequence: *sequence,
            dispatch: id(dispatch_id),
            binding: binding(value),
            thread: id(thread_id),
        }),
        Input::ProjectOutputRecorded {
            project_id,
            output_id,
            dispatch_id,
            binding: value,
            thread_id,
            message,
        } => Output::ProjectOutputRecorded(model::ProjectOutputRecordedDto {
            project: id(project_id),
            output: id(output_id),
            dispatch: id(dispatch_id),
            binding: binding(value),
            thread: id(thread_id),
            message: message_dto(message),
        }),
        Input::RemoteProjectCommandRequested {
            command_id,
            digest,
            project_id,
            target_home,
            expected_head,
            operation: value,
            body,
        } => Output::RemoteProjectCommandRequested(model::RemoteProjectCommandRequestedDto {
            command: id(command_id),
            digest: id(digest),
            project: id(project_id),
            target_home: id(target_home),
            expected_head: model::RequiredOption(expected_head.as_ref().map(id)),
            operation: operation(value),
            body: content_text(body),
        }),
        Input::RemoteProjectCommandReceipt {
            command_id,
            digest,
            project_id,
            received_head,
            received_at,
        } => Output::RemoteProjectCommandReceipt(model::RemoteProjectCommandReceiptDto {
            command: id(command_id),
            digest: id(digest),
            project: id(project_id),
            received_head: model::RequiredOption(received_head.as_ref().map(id)),
            received_at: milliseconds(*received_at),
        }),
        Input::RemoteProjectCommandOutcome {
            command_id,
            digest,
            project_id,
            result,
            runtime,
        } => Output::RemoteProjectCommandOutcome(model::RemoteProjectCommandOutcomeDto {
            command: id(command_id),
            digest: id(digest),
            project: id(project_id),
            result: remote_result(result),
            runtime: optional_runtime(runtime.as_ref()),
        }),
    }
}

fn id<T>(value: &T) -> model::Hex32
where
    T: AsBytes32,
{
    model::Hex32(*value.bytes())
}

trait AsBytes32 {
    fn bytes(&self) -> &[u8; 32];
}

macro_rules! bytes32 {
    ($($type:ty),+ $(,)?) => {
        $(impl AsBytes32 for $type {
            fn bytes(&self) -> &[u8; 32] { self.as_bytes() }
        })+
    };
}

bytes32!(
    domain::AccountId,
    domain::AgentId,
    domain::AssignmentId,
    domain::CommandDigest,
    domain::CommandId,
    domain::DispatchId,
    domain::EncryptionPublicKey,
    domain::FactId,
    domain::GrantId,
    domain::InstallationId,
    domain::MailboxId,
    domain::MessageId,
    domain::OperationId,
    domain::ProjectId,
    domain::ResourceId,
    domain::SigningPublicKey,
    domain::ThreadId,
);

fn role_dto(value: domain::AuthorityRole) -> model::RoleDto {
    match value {
        domain::AuthorityRole::AccountCreator => model::RoleDto::AccountCreator,
        domain::AuthorityRole::AccountMembership => model::RoleDto::AccountMembership,
        domain::AuthorityRole::ActiveHuman => model::RoleDto::ActiveHuman,
        domain::AuthorityRole::Assignment => model::RoleDto::Assignment,
        domain::AuthorityRole::DeviceGrant => model::RoleDto::DeviceGrant,
        domain::AuthorityRole::Dispatch => model::RoleDto::Dispatch,
        domain::AuthorityRole::LocalInstallation => model::RoleDto::LocalInstallation,
        domain::AuthorityRole::MailboxGrant => model::RoleDto::MailboxGrant,
        domain::AuthorityRole::MailboxOwner => model::RoleDto::MailboxOwner,
        domain::AuthorityRole::OutputBinding => model::RoleDto::OutputBinding,
        domain::AuthorityRole::PreviousState => model::RoleDto::PreviousState,
        domain::AuthorityRole::ProjectHome => model::RoleDto::ProjectHome,
        domain::AuthorityRole::Request => model::RoleDto::Request,
    }
}

fn short(value: &domain::ShortText) -> model::ShortText {
    short_string(value.as_str())
}

fn short_string(value: &str) -> model::ShortText {
    model::Text(value.to_owned())
}

fn content_text(value: &domain::ContentText) -> model::ContentText {
    model::Text(value.as_str().to_owned())
}

fn optional_short(value: Option<&domain::ShortText>) -> model::RequiredOption<model::ShortText> {
    model::RequiredOption(value.map(short))
}

fn optional_content(
    value: Option<&domain::ContentText>,
) -> model::RequiredOption<model::ContentText> {
    model::RequiredOption(value.map(content_text))
}

fn provider_text(value: &domain::ProviderId) -> model::ProviderText {
    model::Text(value.as_str().to_owned())
}

fn session_text(value: &domain::ProviderSessionId) -> model::SessionText {
    model::Text(value.as_str().to_owned())
}

fn installation_address(value: domain::InstallationAddress) -> model::InstallationAddressDto {
    model::InstallationAddressDto {
        installation: id(&value.installation_id()),
        signing: id(&value.signing_key()),
    }
}

fn mailbox_address(value: domain::MailboxAddress) -> model::MailboxAddressDto {
    model::MailboxAddressDto {
        installation: id(&value.installation_id()),
        mailbox: id(&value.mailbox_id()),
    }
}

fn locator(value: &domain::ResourceLocator) -> model::LocatorDto {
    model::LocatorDto {
        scheme: match value.scheme() {
            domain::ResourceScheme::GitRepository => model::LocatorSchemeDto::Git,
            domain::ResourceScheme::WorkingTree => model::LocatorSchemeDto::Worktree,
            domain::ResourceScheme::Container => model::LocatorSchemeDto::Container,
            domain::ResourceScheme::Opaque => model::LocatorSchemeDto::Opaque,
        },
        value: model::Text(value.value().to_owned()),
    }
}

fn context_dto(value: &domain::RepositoryContext) -> model::ContextDto {
    model::ContextDto {
        directory: locator(&value.directory),
        repository: model::RequiredOption(value.repository.as_ref().map(locator)),
        worktree: model::RequiredOption(value.worktree.as_ref().map(locator)),
        branch: optional_short(value.branch.as_ref()),
    }
}

fn operation(value: &domain::OperationCorrelation) -> model::OperationDto {
    model::OperationDto {
        provider: provider_text(value.provider()),
        session: session_text(value.session()),
        id: id(&value.operation()),
    }
}

fn message_dto(value: &domain::MessageContent) -> model::MessageDto {
    model::MessageDto {
        id: id(&value.message_id),
        sender: mailbox_address(value.sender),
        recipient: model::RequiredOption(value.recipient.map(mailbox_address)),
        body: content_text(&value.body),
        purpose: match value.purpose {
            domain::MessagePurpose::Question => model::MessagePurposeDto::Question,
            domain::MessagePurpose::Asynchronous => model::MessagePurposeDto::Asynchronous,
            domain::MessagePurpose::ProjectOutput => model::MessagePurposeDto::ProjectOutput,
        },
        presentation: match value.presentation {
            domain::PresentationKind::Message => model::PresentationDto::Message,
            domain::PresentationKind::FinalAnswer => model::PresentationDto::FinalAnswer,
            domain::PresentationKind::Status => model::PresentationDto::Status,
        },
        correlation: model::RequiredOption(value.correlation.as_ref().map(operation)),
        project: model::RequiredOption(value.project_id.as_ref().map(id)),
    }
}

fn resource(value: &domain::ProjectResource) -> model::ResourceDto {
    model::ResourceDto {
        id: id(&value.resource_id),
        display: locator(&value.display_locator),
        canonical: locator(&value.canonical_locator),
        health: resource_health(value.health),
    }
}

fn resource_health(value: domain::ResourceHealth) -> model::ResourceHealthDto {
    match value {
        domain::ResourceHealth::Unknown => model::ResourceHealthDto::Unknown,
        domain::ResourceHealth::Healthy => model::ResourceHealthDto::Healthy,
        domain::ResourceHealth::Degraded => model::ResourceHealthDto::Degraded,
        domain::ResourceHealth::Unavailable => model::ResourceHealthDto::Unavailable,
    }
}

fn binding(value: &domain::AssignmentBinding) -> model::BindingDto {
    model::BindingDto {
        assignment: id(&value.assignment_id),
        agent: id(&value.agent_id),
        provider: provider_text(&value.provider),
        session: session_text(&value.session),
    }
}

fn activity_kind(value: domain::ActivityKind) -> model::ActivityKindDto {
    match value {
        domain::ActivityKind::Status => model::ActivityKindDto::Status,
        domain::ActivityKind::Progress => model::ActivityKindDto::Progress,
        domain::ActivityKind::Plan => model::ActivityKindDto::Plan,
        domain::ActivityKind::Diff => model::ActivityKindDto::Diff,
        domain::ActivityKind::CompletedItem => model::ActivityKindDto::CompletedItem,
    }
}

fn activity_status(value: &domain::ActivityStatus) -> model::ActivityStatusDto {
    let simple = |state| model::ActivityStatusDto::Simple(model::ActivitySimpleStatusDto { state });
    match value {
        domain::ActivityStatus::Snapshot => simple(model::ActivitySimpleStateDto::Snapshot),
        domain::ActivityStatus::Running => simple(model::ActivitySimpleStateDto::Running),
        domain::ActivityStatus::Succeeded => simple(model::ActivitySimpleStateDto::Succeeded),
        domain::ActivityStatus::Interrupted => simple(model::ActivitySimpleStateDto::Interrupted),
        domain::ActivityStatus::Failed(code) => {
            model::ActivityStatusDto::Failed(model::FailedStatusDto {
                state: model::FailedStateTag::Failed,
                code: short_string(code.as_str()),
            })
        }
    }
}

fn runtime(value: &domain::RuntimeObservation) -> model::RuntimeDto {
    match value {
        domain::RuntimeObservation::Succeeded => {
            model::RuntimeDto::Succeeded(model::SucceededRuntimeDto {
                state: model::SucceededStateTag::Succeeded,
            })
        }
        domain::RuntimeObservation::Failed(code) => {
            model::RuntimeDto::Failed(model::FailedRuntimeDto {
                state: model::FailedStateTag::Failed,
                code: short_string(code.as_str()),
            })
        }
        domain::RuntimeObservation::Uncertain(code) => {
            model::RuntimeDto::Uncertain(model::UncertainRuntimeDto {
                state: model::UncertainStateTag::Uncertain,
                code: short_string(code.as_str()),
            })
        }
    }
}

fn optional_runtime(
    value: Option<&domain::RuntimeObservation>,
) -> model::RequiredOption<model::RuntimeDto> {
    model::RequiredOption(value.map(runtime))
}

fn remote_result(value: &domain::RemoteCommandResult) -> model::RemoteResultDto {
    match value {
        domain::RemoteCommandResult::Committed(head) => {
            model::RemoteResultDto::Committed(model::CommittedResultDto {
                state: model::CommittedStateTag::Committed,
                head: id(head),
            })
        }
        domain::RemoteCommandResult::Rejected {
            error,
            external_state_warning,
        } => model::RemoteResultDto::Rejected(model::RejectedResultDto {
            state: model::RejectedStateTag::Rejected,
            code: short_string(error.as_str()),
            external_state_warning: model::RequiredOption(external_state_warning.as_ref().map(
                |warning| match warning {
                    domain::ProjectExternalStateWarning::WorktreeMayExist {
                        destination,
                        branch,
                    } => model::ExternalStateWarningDto {
                        kind: model::ExternalStateWarningKindDto::WorktreeMayExist,
                        destination: locator(destination),
                        branch: short_string(branch.as_str()),
                    },
                },
            )),
        }),
    }
}

fn mailbox_kind(value: domain::MailboxKind) -> model::MailboxKindDto {
    match value {
        domain::MailboxKind::Human => model::MailboxKindDto::Human,
        domain::MailboxKind::Agent => model::MailboxKindDto::Agent,
    }
}

fn initial_state_dto(value: domain::InitialProjectState) -> model::InitialStateDto {
    match value {
        domain::InitialProjectState::Open => model::InitialStateDto::Open,
        domain::InitialProjectState::Closed => model::InitialStateDto::Closed,
    }
}

fn project_target(project_id: &domain::ProjectId) -> model::ProjectTargetDto {
    model::ProjectTargetDto {
        project: id(project_id),
    }
}

fn milliseconds(value: domain::Timestamp) -> model::Milliseconds {
    model::Milliseconds(u64::try_from(value.as_unix_millis()).unwrap_or(u64::MAX))
}
