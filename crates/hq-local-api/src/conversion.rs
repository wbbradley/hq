//! Exhaustive local API v1 conversion boundary.

use crate::protocol::v1::{
    AgentSessionRequestDto, AgentSessionResultDto, AuthoritativeSnapshotDto, ConversationEntryDto,
    ConversationKeyDto, ConversationPageDto, ConversationPageRequest, DomainErrorDto,
    EffectOutcomeDto, EffectRequestDto, ErrorClass, ErrorResponse, Id32, InvalidationTopic,
    MutationAttemptDto, MutationOutcomeDto, MutationRequest, RelayAccessDto,
    RelayAuthenticationDto, RelayConfigurationDto, ResourceHealthDto, ResourceInspectionRequestDto,
    ResourceInspectionResultDto, ResourceLocatorDto, ResourceSchemeDto, SessionControlDto,
    SnapshotItem, SubscriptionRequestDto, SynchronizationRequestDto, ValueError,
};
use hq_application::{
    AgentSessionRequest, AgentSessionResult, ApplicationError, ApplicationErrorClass,
    AuthoritativeSnapshot, ClientAgentLifecycle, ClientMembershipState, ClientPeerRouteState,
    ClientProjectLifecycle, ClientProjectOutputStatus, ClientProjection, ClientRemoteCommandStage,
    ConversationEntry, ConversationKey, EffectOutcome, EffectRequest, FactMutation,
    MutationAttempt, MutationDecision, MutationOutcome, MutationReceipt, RelayAccess,
    RelayAuthentication, RelayConfiguration, ResourceInspectionRequest, ResourceInspectionResult,
    SessionControl, SubscriptionRequest, SubscriptionTopic, SynchronizationRequest,
};
use hq_domain::{
    ActivityStatus, AgentId, BoundedText, CommandDigest, CommandId, ErrorCategory, MailboxAddress,
    OperationId, Page, PageCursor, ProjectId, ProviderId, ProviderSessionId,
    RESOURCE_LOCATOR_MAX_BYTES, ResourceHealth, ResourceId, ResourceLocator, ResourceScheme,
    Revision, Timestamp,
};

/// Converts one complete normalized application snapshot into bounded local API v1 items.
#[allow(clippy::too_many_lines)]
pub fn snapshot_to_v1(
    snapshot: &AuthoritativeSnapshot,
) -> Result<AuthoritativeSnapshotDto, ValueError> {
    let projections = snapshot
        .client_projections()
        .map_err(|_| ValueError::InvalidValueCombination)?;
    let items = projections
        .into_iter()
        .map(|projection| match projection {
            ClientProjection::Installation {
                installation_id,
                signing_key,
                encryption_key,
                label,
            } => SnapshotItem::Installation {
                installation_id: id32(installation_id.as_bytes()),
                signing_key: id32(signing_key.as_bytes()),
                encryption_key: id32(encryption_key.as_bytes()),
                label: label.map(|value| value.as_str().to_owned()),
            },
            ClientProjection::Mailbox {
                address,
                kind,
                label,
            } => SnapshotItem::Mailbox {
                installation_id: id32(address.installation_id().as_bytes()),
                mailbox_id: id32(address.mailbox_id().as_bytes()),
                mailbox_kind: match kind {
                    hq_domain::MailboxKind::Human => "human",
                    hq_domain::MailboxKind::Agent => "agent",
                }
                .to_owned(),
                label: label.map(|value| value.as_str().to_owned()),
            },
            ClientProjection::Account {
                account_id,
                creator_installation,
                label,
                selected,
            } => SnapshotItem::Account {
                account_id: id32(account_id.as_bytes()),
                creator_installation: id32(creator_installation.as_bytes()),
                label: label.map(|value| value.as_str().to_owned()),
                selected,
            },
            ClientProjection::PeerRoute {
                owner,
                peer,
                state,
                frontier,
            } => SnapshotItem::PeerRoute {
                owner: id32(owner.as_bytes()),
                peer: id32(peer.as_bytes()),
                state: match state {
                    ClientPeerRouteState::Routable => "routable",
                    ClientPeerRouteState::Blocked => "blocked",
                    ClientPeerRouteState::Conflicted => "conflicted",
                }
                .to_owned(),
                frontier: frontier.iter().map(|fact| id32(fact.as_bytes())).collect(),
            },
            ClientProjection::MailboxCapability {
                grant_id,
                mailbox,
                grantee_installation,
                active,
            } => SnapshotItem::MailboxCapability {
                grant_id: id32(grant_id.as_bytes()),
                mailbox_installation: id32(mailbox.installation_id().as_bytes()),
                mailbox_id: id32(mailbox.mailbox_id().as_bytes()),
                grantee_installation: id32(grantee_installation.as_bytes()),
                active,
            },
            ClientProjection::Membership {
                account_id,
                device,
                state,
                active_acceptances,
            } => SnapshotItem::Membership {
                account_id: id32(account_id.as_bytes()),
                device: id32(device.as_bytes()),
                state: match state {
                    ClientMembershipState::Pending => "pending",
                    ClientMembershipState::Active => "active",
                    ClientMembershipState::Revoked => "revoked",
                }
                .to_owned(),
                active_acceptances: active_acceptances
                    .iter()
                    .map(|fact| id32(fact.as_bytes()))
                    .collect(),
            },
            ClientProjection::AccountSelection {
                installation_id,
                candidates,
                active,
            } => SnapshotItem::AccountSelection {
                installation_id: id32(installation_id.as_bytes()),
                candidates: candidates
                    .iter()
                    .map(|account| id32(account.as_bytes()))
                    .collect(),
                active: active.map(|account| id32(account.as_bytes())),
            },
            ClientProjection::Conversation {
                key,
                latest_fact,
                open_messages,
            } => SnapshotItem::Conversation {
                key: conversation_key_to_v1(&key),
                latest_fact: latest_fact.map(|fact| id32(fact.as_bytes())),
                open_messages,
            },
            ClientProjection::Agent {
                agent_id,
                names,
                lifecycle,
                runnable,
            } => SnapshotItem::Agent {
                agent_id: id32(agent_id.as_bytes()),
                names: names.iter().map(|name| name.as_str().to_owned()).collect(),
                lifecycle: match lifecycle {
                    ClientAgentLifecycle::Active => "active",
                    ClientAgentLifecycle::Conflicted => "conflicted",
                    ClientAgentLifecycle::Retired => "retired",
                }
                .to_owned(),
                runnable,
            },
            ClientProjection::AgentSession {
                provider,
                session,
                mailbox,
                conflicted,
            } => SnapshotItem::AgentSession {
                provider: provider.as_str().to_owned(),
                session: session.as_str().to_owned(),
                mailbox_installation: mailbox
                    .map(|address| id32(address.installation_id().as_bytes())),
                mailbox_id: mailbox.map(|address| id32(address.mailbox_id().as_bytes())),
                conflicted,
            },
            ClientProjection::AgentSelection {
                agent_id,
                selected,
                conflicted,
            } => {
                let (provider, session) = selected.map_or((None, None), |(provider, session)| {
                    (
                        Some(provider.as_str().to_owned()),
                        Some(session.as_str().to_owned()),
                    )
                });
                SnapshotItem::AgentSelection {
                    agent_id: id32(agent_id.as_bytes()),
                    provider,
                    session,
                    conflicted,
                }
            }
            ClientProjection::AgentSessionName {
                agent_id,
                provider,
                session,
                resolved,
                display_name,
            } => SnapshotItem::AgentSessionName {
                agent_id: id32(agent_id.as_bytes()),
                provider: provider.as_str().to_owned(),
                session: session.as_str().to_owned(),
                resolved,
                display_name: display_name.map(|name| name.as_str().to_owned()),
            },
            ClientProjection::Project {
                project_id,
                home,
                name,
                lifecycle,
                archived,
                claimable,
                head,
                input_sequence,
            } => SnapshotItem::Project {
                project_id: id32(project_id.as_bytes()),
                home: id32(home.as_bytes()),
                name: name.as_str().to_owned(),
                lifecycle: match lifecycle {
                    ClientProjectLifecycle::Open => "open",
                    ClientProjectLifecycle::Closing => "closing",
                    ClientProjectLifecycle::Closed => "closed",
                }
                .to_owned(),
                archived,
                claimable,
                head: id32(head.as_bytes()),
                input_sequence,
            },
            ClientProjection::ProjectInput {
                project_id,
                message_id,
                sequence,
                accepted_fact,
            } => SnapshotItem::ProjectInput {
                project_id: id32(project_id.as_bytes()),
                message_id: id32(message_id.as_bytes()),
                sequence,
                accepted_fact: id32(accepted_fact.as_bytes()),
            },
            ClientProjection::ProjectDispatch {
                dispatch_id,
                message_id,
                sequence,
                fact_id,
                conflicted,
            } => SnapshotItem::ProjectDispatch {
                dispatch_id: id32(dispatch_id.as_bytes()),
                message_id: id32(message_id.as_bytes()),
                sequence,
                fact_id: id32(fact_id.as_bytes()),
                conflicted,
            },
            ClientProjection::ProjectOutput {
                output_id,
                dispatch_id,
                status,
                content,
            } => SnapshotItem::ProjectOutput {
                output_id: id32(output_id.as_bytes()),
                dispatch_id: id32(dispatch_id.as_bytes()),
                status: match status {
                    ClientProjectOutputStatus::Current => "current",
                    ClientProjectOutputStatus::LateFromInactive => "late_from_inactive",
                    ClientProjectOutputStatus::Conflicted => "conflicted",
                }
                .to_owned(),
                content: content.as_str().to_owned(),
            },
            ClientProjection::RemoteCommand {
                command_id,
                request_digest,
                project_id,
                stage,
            } => SnapshotItem::RemoteCommand {
                command_id: id32(command_id.as_bytes()),
                request_digest: id32(request_digest.as_bytes()),
                project_id: id32(project_id.as_bytes()),
                stage: match stage {
                    ClientRemoteCommandStage::Queued => "queued",
                    ClientRemoteCommandStage::Received => "received",
                    ClientRemoteCommandStage::Terminal => "terminal",
                    ClientRemoteCommandStage::Conflicted => "conflicted",
                }
                .to_owned(),
            },
        })
        .collect();
    AuthoritativeSnapshotDto::new(snapshot.revision().value(), items)
}

/// Converts one bounded local page request into application query values.
pub fn page_request_from_v1(
    request: ConversationPageRequest,
) -> Result<(ConversationKey, usize, Option<PageCursor>), ValueError> {
    let key = conversation_key_from_v1(request.key)?;
    let cursor = request
        .cursor
        .map(PageCursor::new)
        .transpose()
        .map_err(|_| ValueError::InvalidCursor)?;
    Ok((key, usize::from(request.limit), cursor))
}

/// Converts one bounded application page into its v1 response.
pub fn page_to_v1(page: &Page<ConversationEntry>) -> Result<ConversationPageDto, ValueError> {
    let items = page.items().iter().map(conversation_entry_to_v1).collect();
    ConversationPageDto::new(
        items,
        page.next_cursor().map(|cursor| cursor.as_str().to_owned()),
    )
}

/// Converts one exact wire mutation into a one-shot application decision.
pub fn mutation_from_v1(request: MutationRequest) -> Result<FactMutation, ValueError> {
    let command_id = request.command_id();
    let request_digest = request.request_digest();
    let plan = request.into_plan()?;
    Ok(FactMutation::new(command_id, request_digest, move |_| {
        MutationDecision::commit(plan)
    }))
}

/// Converts one application mutation attempt into its retry-safe v1 result.
pub fn mutation_to_v1(attempt: &MutationAttempt) -> MutationAttemptDto {
    match attempt {
        MutationAttempt::Completed(receipt) => mutation_receipt_to_v1(receipt),
        MutationAttempt::Uncertain {
            command_id,
            request_digest,
        } => MutationAttemptDto::Uncertain {
            command_id: id32(command_id.as_bytes()),
            request_digest: id32(request_digest.as_bytes()),
        },
    }
}

/// Converts a wire subscription into the application observer vocabulary.
pub fn subscription_from_v1(
    request: SubscriptionRequestDto,
) -> Result<SubscriptionRequest, ValueError> {
    SubscriptionRequest::new(
        OperationId::from_bytes(request.subscription_id.bytes()),
        request.topics.into_iter().map(topic_from_v1),
    )
    .map_err(|_| ValueError::InvalidTopics)
}

/// Converts one application notice topic into the stable v1 spelling.
pub const fn topic_to_v1(topic: SubscriptionTopic) -> InvalidationTopic {
    match topic {
        SubscriptionTopic::All => InvalidationTopic::All,
        SubscriptionTopic::Authority => InvalidationTopic::Authority,
        SubscriptionTopic::Conversation => InvalidationTopic::Conversation,
        SubscriptionTopic::Agent => InvalidationTopic::Agent,
        SubscriptionTopic::Project => InvalidationTopic::Project,
        SubscriptionTopic::Operations => InvalidationTopic::Operations,
    }
}

/// Converts a v1 topic into the application observer vocabulary.
pub const fn topic_from_v1(topic: InvalidationTopic) -> SubscriptionTopic {
    match topic {
        InvalidationTopic::All => SubscriptionTopic::All,
        InvalidationTopic::Authority => SubscriptionTopic::Authority,
        InvalidationTopic::Conversation => SubscriptionTopic::Conversation,
        InvalidationTopic::Agent => SubscriptionTopic::Agent,
        InvalidationTopic::Project => SubscriptionTopic::Project,
        InvalidationTopic::Operations => SubscriptionTopic::Operations,
    }
}

/// Converts a redacted application error into the closed v1 error vocabulary.
pub fn application_error_to_v1(error: ApplicationError) -> ErrorResponse {
    let class = match error.class() {
        ApplicationErrorClass::InvalidInput => ErrorClass::InvalidInput,
        ApplicationErrorClass::Conflict => ErrorClass::Conflict,
        ApplicationErrorClass::Unauthorized => ErrorClass::Unauthorized,
        ApplicationErrorClass::Unresolved | ApplicationErrorClass::NotFound => ErrorClass::NotFound,
        ApplicationErrorClass::Capacity | ApplicationErrorClass::Unavailable => {
            ErrorClass::Unavailable
        }
        ApplicationErrorClass::CorruptState | ApplicationErrorClass::InvariantViolation => {
            ErrorClass::Internal
        }
    };
    ErrorResponse {
        class,
        code: error.code().as_str().to_owned(),
        detail: None,
    }
}

pub(crate) fn relay_effect_from_v1(
    request: &EffectRequestDto<RelayConfigurationDto>,
) -> Result<EffectRequest<RelayConfiguration>, ValueError> {
    let body = RelayConfiguration::new(
        locator_from_v1(request.body.endpoint.clone())?,
        match request.body.access {
            RelayAccessDto::Read => RelayAccess::Read,
            RelayAccessDto::Write => RelayAccess::Write,
            RelayAccessDto::ReadWrite => RelayAccess::ReadWrite,
        },
        match request.body.authentication {
            RelayAuthenticationDto::Disabled => RelayAuthentication::Disabled,
            RelayAuthenticationDto::OnChallenge => RelayAuthentication::OnChallenge,
            RelayAuthenticationDto::Required => RelayAuthentication::Required,
        },
    );
    Ok(effect_from_v1(request, body))
}

pub(crate) fn synchronization_effect_from_v1(
    request: &EffectRequestDto<SynchronizationRequestDto>,
) -> Result<EffectRequest<SynchronizationRequest>, ValueError> {
    let body = match request.body.clone() {
        SynchronizationRequestDto::All => SynchronizationRequest::All,
        SynchronizationRequestDto::Relay(locator) => {
            SynchronizationRequest::Relay(locator_from_v1(locator)?)
        }
    };
    Ok(effect_from_v1(request, body))
}

pub(crate) fn agent_effect_from_v1(
    request: &EffectRequestDto<AgentSessionRequestDto>,
) -> Result<EffectRequest<AgentSessionRequest>, ValueError> {
    let body = AgentSessionRequest::new(
        AgentId::from_bytes(request.body.agent_id.bytes()),
        ProviderId::new(request.body.provider.clone()).map_err(|_| ValueError::InvalidText)?,
        match request.body.control.clone() {
            SessionControlDto::Start => SessionControl::Start,
            SessionControlDto::Resume(session) => SessionControl::Resume {
                session: ProviderSessionId::new(session).map_err(|_| ValueError::InvalidText)?,
            },
            SessionControlDto::Stop => SessionControl::Stop,
        },
    );
    Ok(effect_from_v1(request, body))
}

pub(crate) fn resource_effect_from_v1(
    request: &EffectRequestDto<ResourceInspectionRequestDto>,
) -> Result<EffectRequest<ResourceInspectionRequest>, ValueError> {
    let body = ResourceInspectionRequest::new(
        ProjectId::from_bytes(request.body.project_id.bytes()),
        ResourceId::from_bytes(request.body.resource_id.bytes()),
        locator_from_v1(request.body.locator.clone())?,
    );
    Ok(effect_from_v1(request, body))
}

pub(crate) fn empty_effect_to_v1(outcome: &EffectOutcome<()>) -> EffectOutcomeDto<()> {
    effect_to_v1(outcome, |()| ())
}

pub(crate) fn agent_effect_to_v1(
    outcome: &EffectOutcome<AgentSessionResult>,
) -> EffectOutcomeDto<AgentSessionResultDto> {
    effect_to_v1(outcome, |result| match result {
        AgentSessionResult::Ready(session) => {
            AgentSessionResultDto::Ready(session.as_str().to_owned())
        }
        AgentSessionResult::Stopped => AgentSessionResultDto::Stopped,
    })
}

pub(crate) fn resource_effect_to_v1(
    outcome: &EffectOutcome<ResourceInspectionResult>,
) -> EffectOutcomeDto<ResourceInspectionResultDto> {
    effect_to_v1(outcome, |result| ResourceInspectionResultDto {
        health: resource_health_to_v1(result.health()),
        details: result.details().map(|details| details.as_str().to_owned()),
        checked_at_unix_millis: result.checked_at().as_unix_millis(),
    })
}

fn effect_from_v1<T, U>(request: &EffectRequestDto<T>, body: U) -> EffectRequest<U> {
    EffectRequest::new(
        OperationId::from_bytes(request.operation_id.bytes()),
        CommandDigest::from_bytes(request.request_digest.bytes()),
        Timestamp::from_unix_millis(request.issued_at_unix_millis),
        body,
    )
}

fn effect_to_v1<T, U>(
    outcome: &EffectOutcome<T>,
    accepted: impl FnOnce(&T) -> U,
) -> EffectOutcomeDto<U> {
    match outcome {
        EffectOutcome::Accepted(value) => EffectOutcomeDto::Accepted(accepted(value)),
        EffectOutcome::Rejected(error) => EffectOutcomeDto::Rejected(domain_error_to_v1(error)),
        EffectOutcome::Uncertain(operation_id) => {
            EffectOutcomeDto::Uncertain(id32(operation_id.as_bytes()))
        }
    }
}

fn mutation_receipt_to_v1(receipt: &MutationReceipt) -> MutationAttemptDto {
    MutationAttemptDto::Completed {
        command_id: id32(receipt.command_id().as_bytes()),
        request_digest: id32(receipt.request_digest().as_bytes()),
        revision: receipt.revision().value(),
        outcome: match receipt.outcome() {
            MutationOutcome::Committed => MutationOutcomeDto::Committed,
            MutationOutcome::Rejected(error) => MutationOutcomeDto::Rejected {
                category: error_category(error.category()).to_owned(),
                code: error.code().as_str().to_owned(),
            },
        },
    }
}

fn conversation_key_from_v1(key: ConversationKeyDto) -> Result<ConversationKey, ValueError> {
    match key {
        ConversationKeyDto::Thread {
            counterparty_installation,
            counterparty_mailbox,
            thread,
        } => Ok(ConversationKey::Thread {
            counterparty: MailboxAddress::new(
                hq_domain::InstallationId::from_bytes(counterparty_installation.bytes()),
                hq_domain::MailboxId::from_bytes(counterparty_mailbox.bytes()),
            ),
            thread: hq_domain::ThreadId::from_bytes(thread.bytes()),
        }),
        ConversationKeyDto::ProviderSession {
            counterparty_installation,
            counterparty_mailbox,
            provider,
            session,
        } => Ok(ConversationKey::ProviderSession {
            counterparty: MailboxAddress::new(
                hq_domain::InstallationId::from_bytes(counterparty_installation.bytes()),
                hq_domain::MailboxId::from_bytes(counterparty_mailbox.bytes()),
            ),
            provider: ProviderId::new(provider).map_err(|_| ValueError::InvalidText)?,
            session: ProviderSessionId::new(session).map_err(|_| ValueError::InvalidText)?,
        }),
    }
}

fn conversation_key_to_v1(key: &ConversationKey) -> ConversationKeyDto {
    match key {
        ConversationKey::Thread {
            counterparty,
            thread,
        } => ConversationKeyDto::Thread {
            counterparty_installation: id32(counterparty.installation_id().as_bytes()),
            counterparty_mailbox: id32(counterparty.mailbox_id().as_bytes()),
            thread: id32(thread.as_bytes()),
        },
        ConversationKey::ProviderSession {
            counterparty,
            provider,
            session,
        } => ConversationKeyDto::ProviderSession {
            counterparty_installation: id32(counterparty.installation_id().as_bytes()),
            counterparty_mailbox: id32(counterparty.mailbox_id().as_bytes()),
            provider: provider.as_str().to_owned(),
            session: session.as_str().to_owned(),
        },
    }
}

fn conversation_entry_to_v1(entry: &ConversationEntry) -> ConversationEntryDto {
    match entry {
        ConversationEntry::Message(message) => ConversationEntryDto::Message {
            fact_id: id32(message.fact_id.as_bytes()),
            message_id: id32(message.content.message_id.as_bytes()),
            thread_id: id32(message.thread_id.as_bytes()),
            content: message.content.body.as_str().to_owned(),
            open: message.open,
            rejected: message.rejected,
        },
        ConversationEntry::Activity(activity) => ConversationEntryDto::Activity {
            fact_id: id32(activity.fact_id.as_bytes()),
            sequence: activity.sequence.get(),
            status: activity_status(&activity.status).to_owned(),
            content: activity.content.as_str().to_owned(),
            truncated: activity.truncated,
        },
    }
}

fn locator_from_v1(locator: ResourceLocatorDto) -> Result<ResourceLocator, ValueError> {
    let value = BoundedText::<RESOURCE_LOCATOR_MAX_BYTES>::new(locator.value)
        .map_err(|_| ValueError::InvalidText)?;
    Ok(ResourceLocator::new(
        match locator.scheme {
            ResourceSchemeDto::GitRepository => ResourceScheme::GitRepository,
            ResourceSchemeDto::WorkingTree => ResourceScheme::WorkingTree,
            ResourceSchemeDto::Container => ResourceScheme::Container,
            ResourceSchemeDto::Opaque => ResourceScheme::Opaque,
        },
        value,
    ))
}

fn domain_error_to_v1(error: &hq_domain::DomainError) -> DomainErrorDto {
    DomainErrorDto {
        category: error_category(error.category()).to_owned(),
        code: error.code().as_str().to_owned(),
    }
}

const fn resource_health_to_v1(health: ResourceHealth) -> ResourceHealthDto {
    match health {
        ResourceHealth::Unknown => ResourceHealthDto::Unknown,
        ResourceHealth::Healthy => ResourceHealthDto::Healthy,
        ResourceHealth::Degraded => ResourceHealthDto::Degraded,
        ResourceHealth::Unavailable => ResourceHealthDto::Unavailable,
    }
}

const fn error_category(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::InvalidInput => "invalid_input",
        ErrorCategory::Conflict => "conflict",
        ErrorCategory::Unauthorized => "unauthorized",
        ErrorCategory::Unresolved => "unresolved",
        ErrorCategory::NotFound => "not_found",
        ErrorCategory::InvariantViolation => "invariant_violation",
    }
}

const fn activity_status(status: &ActivityStatus) -> &'static str {
    match status {
        ActivityStatus::Snapshot => "snapshot",
        ActivityStatus::Running => "running",
        ActivityStatus::Succeeded => "succeeded",
        ActivityStatus::Failed(_) => "failed",
        ActivityStatus::Interrupted => "interrupted",
    }
}

fn id32(bytes: &[u8; 32]) -> Id32 {
    Id32::new(*bytes)
}

#[allow(dead_code)]
fn _pin_wire_resource_locator(_value: ResourceLocatorDto) {}

#[allow(dead_code)]
fn _pin_wire_ids(_command_id: CommandId, _operation_id: OperationId, _revision: Revision) {}
