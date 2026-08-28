//! Exhaustive local API v1 conversion boundary.

use crate::protocol::v1::{
    AgentSessionRequestDto, AgentSessionResultDto, AuthoritativeSnapshotDto, CanonicalEvidenceDto,
    ConversationEntryDto, ConversationKeyDto, ConversationPageDto, ConversationPageRequest,
    DeviceGrantDto, DomainErrorDto, DomainHealthDto, EffectOutcomeDto, EffectRequestDto,
    ErrorClass, ErrorResponse, EvidenceIngestOutcomeDto, HealthDomainDto, Id32, InvalidationTopic,
    MAX_CANONICAL_EVIDENCE_BYTES, MAX_CANONICAL_EVIDENCE_ITEMS, MutationAttemptDto,
    MutationOutcomeDto, MutationRequest, PeerRouteBlockDto, PeerRouteCandidateDto,
    ProjectCommandActionDto, ProjectCommandOutcomeDto, ProjectCommandRequestDto,
    ProjectCommandStageDto, ProjectResourceDto, RelayAccessDto, RelayAuthenticationDto,
    RelayConfigurationDto, RelayPolicyStatusDto, RelayStatusDto, RemoteCommandProgressDto,
    RemoteCommandResultDto, ResourceHealthDto, ResourceInspectionRequestDto,
    ResourceInspectionResultDto, ResourceLocatorDto, ResourceSchemeDto, RuntimeObservationDto,
    SessionControlDto, SnapshotItem, StateHealthDto, StateRepairReportDto, SubscriptionRequestDto,
    SynchronizationRequestDto, ValueError,
};
use hq_application::{
    AgentSessionRequest, AgentSessionResult, ApplicationError, ApplicationErrorClass,
    AuthoritativeSnapshot, CanonicalEvidence, ClientAgentLifecycle, ClientMembershipState,
    ClientPeerRouteState, ClientProjectLifecycle, ClientProjectOutputStatus, ClientProjection,
    ClientRemoteCommandStage, ConversationEntry, ConversationKey, DomainHealth, EffectOutcome,
    EffectRequest, EvidenceIngestOutcome, FactMutation, HealthDomain, MutationAttempt,
    MutationDecision, MutationOutcome, MutationReceipt, ProjectCommandAction,
    ProjectCommandOutcome, ProjectCommandRequest, ProjectCommandStage, RelayAccess,
    RelayAuthentication, RelayConfiguration, RelayStatus, ResourceInspectionRequest,
    ResourceInspectionResult, SessionControl, StateHealth, StateRepairReport, SubscriptionRequest,
    SubscriptionTopic, SynchronizationRequest, WorktreeProvisioningRequest,
};
use hq_domain::{
    ActivityStatus, AgentId, BoundedText, CommandDigest, CommandId, ErrorCategory, FactId,
    MailboxAddress, MailboxId, OperationId, Page, PageCursor, ProjectId, ProjectResource,
    ProviderId, ProviderSessionId, RESOURCE_LOCATOR_MAX_BYTES, RemoteCommandResult, ResourceHealth,
    ResourceId, ResourceLocator, ResourceScheme, Revision, RuntimeObservation, ShortText, ThreadId,
    Timestamp,
};

/// Converts bounded exact canonical evidence into its wire representation.
pub fn canonical_evidence_to_v1(
    evidence: &[CanonicalEvidence],
) -> Result<Vec<CanonicalEvidenceDto>, ValueError> {
    if evidence.is_empty() || evidence.len() > MAX_CANONICAL_EVIDENCE_ITEMS {
        return Err(ValueError::TooManyItems);
    }
    let mut total = 0_usize;
    let mut previous = None;
    evidence
        .iter()
        .map(|item| {
            if previous.is_some_and(|value| value >= item.fact_id) {
                return Err(ValueError::InvalidValueCombination);
            }
            total = total
                .checked_add(item.exact_event.len())
                .ok_or(ValueError::TooManyItems)?;
            if total > MAX_CANONICAL_EVIDENCE_BYTES {
                return Err(ValueError::TooManyItems);
            }
            previous = Some(item.fact_id);
            Ok(CanonicalEvidenceDto {
                fact_id: id32(item.fact_id.as_bytes()),
                exact_event: String::from_utf8(item.exact_event.clone())
                    .map_err(|_| ValueError::InvalidText)?,
            })
        })
        .collect()
}

/// Converts bounded wire evidence into passive application values for re-verification.
pub fn canonical_evidence_from_v1(
    evidence: Vec<CanonicalEvidenceDto>,
) -> Result<Vec<CanonicalEvidence>, ValueError> {
    if evidence.is_empty() || evidence.len() > MAX_CANONICAL_EVIDENCE_ITEMS {
        return Err(ValueError::TooManyItems);
    }
    let mut total = 0_usize;
    let mut previous = None;
    evidence
        .into_iter()
        .map(|item| {
            let fact_id = FactId::from_bytes(item.fact_id.bytes());
            if previous.is_some_and(|value| value >= fact_id) || item.exact_event.is_empty() {
                return Err(ValueError::InvalidValueCombination);
            }
            total = total
                .checked_add(item.exact_event.len())
                .ok_or(ValueError::TooManyItems)?;
            if total > MAX_CANONICAL_EVIDENCE_BYTES {
                return Err(ValueError::TooManyItems);
            }
            previous = Some(fact_id);
            Ok(CanonicalEvidence {
                fact_id,
                exact_event: item.exact_event.into_bytes(),
            })
        })
        .collect()
}

/// Converts passive evidence-import outcomes to local API v1.
pub fn evidence_ingest_to_v1(outcomes: &[EvidenceIngestOutcome]) -> Vec<EvidenceIngestOutcomeDto> {
    outcomes
        .iter()
        .map(|outcome| EvidenceIngestOutcomeDto {
            fact_id: id32(outcome.fact_id.as_bytes()),
            revision: outcome.revision.value(),
            inserted: outcome.inserted,
        })
        .collect()
}

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
                root_fact,
                signing_key,
                encryption_key,
                label,
            } => SnapshotItem::Installation {
                installation_id: id32(installation_id.as_bytes()),
                root_fact: id32(root_fact.as_bytes()),
                signing_key: id32(signing_key.as_bytes()),
                encryption_key: id32(encryption_key.as_bytes()),
                label: label.map(|value| value.as_str().to_owned()),
            },
            ClientProjection::Mailbox {
                address,
                create_fact,
                kind,
                label,
            } => SnapshotItem::Mailbox {
                installation_id: id32(address.installation_id().as_bytes()),
                mailbox_id: id32(address.mailbox_id().as_bytes()),
                create_fact: id32(create_fact.as_bytes()),
                mailbox_kind: match kind {
                    hq_domain::MailboxKind::Human => "human",
                    hq_domain::MailboxKind::Agent => "agent",
                }
                .to_owned(),
                label: label.map(|value| value.as_str().to_owned()),
            },
            ClientProjection::Account {
                account_id,
                root_fact,
                creator_installation,
                label,
                selected,
            } => SnapshotItem::Account {
                account_id: id32(account_id.as_bytes()),
                root_fact: id32(root_fact.as_bytes()),
                creator_installation: id32(creator_installation.as_bytes()),
                label: label.map(|value| value.as_str().to_owned()),
                selected,
            },
            ClientProjection::PeerRoute {
                owner,
                peer,
                state,
                frontier,
                routes,
                blocks,
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
                routes: routes
                    .iter()
                    .map(|route| PeerRouteCandidateDto {
                        fact_id: id32(route.fact_id.as_bytes()),
                        signing_key: id32(route.peer.signing_key().as_bytes()),
                        encryption_key: id32(route.encryption_key.as_bytes()),
                        label: route.label.as_ref().map(|label| label.as_str().to_owned()),
                        relay_hints: route
                            .relay_hints
                            .as_slice()
                            .iter()
                            .map(locator_to_v1)
                            .collect(),
                        frontier_member: route.frontier_member,
                    })
                    .collect(),
                blocks: blocks
                    .iter()
                    .map(|block| PeerRouteBlockDto {
                        fact_id: id32(block.fact_id.as_bytes()),
                        reason: block.reason.as_str().to_owned(),
                        frontier_member: block.frontier_member,
                    })
                    .collect(),
            },
            ClientProjection::MailboxCapability {
                grant_id,
                grant_fact,
                mailbox,
                grantee,
                active,
                revoke_frontier,
                observed_actions,
                support,
            } => SnapshotItem::MailboxCapability {
                grant_id: id32(grant_id.as_bytes()),
                grant_fact: id32(grant_fact.as_bytes()),
                mailbox_installation: id32(mailbox.installation_id().as_bytes()),
                mailbox_id: id32(mailbox.mailbox_id().as_bytes()),
                grantee_installation: id32(grantee.installation_id().as_bytes()),
                grantee_signing_key: id32(grantee.signing_key().as_bytes()),
                active,
                revoke_frontier: revoke_frontier
                    .iter()
                    .map(|fact| id32(fact.as_bytes()))
                    .collect(),
                observed_actions: observed_actions
                    .iter()
                    .map(|fact| id32(fact.as_bytes()))
                    .collect(),
                support: support.iter().map(|fact| id32(fact.as_bytes())).collect(),
            },
            ClientProjection::Membership {
                account_id,
                device,
                state,
                frontier,
                grants,
                acceptances,
                revokes,
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
                frontier: frontier.iter().map(|fact| id32(fact.as_bytes())).collect(),
                grants: grants
                    .iter()
                    .map(|grant| DeviceGrantDto {
                        grant_id: id32(grant.grant_id.as_bytes()),
                        grant_fact: id32(grant.grant_fact.as_bytes()),
                        device: id32(grant.device.installation_id().as_bytes()),
                        signing_key: id32(grant.device.signing_key().as_bytes()),
                        label: grant.label.as_ref().map(|label| label.as_str().to_owned()),
                        relay_hints: grant
                            .relay_hints
                            .as_slice()
                            .iter()
                            .map(locator_to_v1)
                            .collect(),
                        frontier_member: grant.frontier_member,
                        active: grant.active,
                    })
                    .collect(),
                acceptances: acceptances
                    .iter()
                    .map(|fact| id32(fact.as_bytes()))
                    .collect(),
                revokes: revokes.iter().map(|fact| id32(fact.as_bytes())).collect(),
                active_acceptances: active_acceptances
                    .iter()
                    .map(|fact| id32(fact.as_bytes()))
                    .collect(),
            },
            ClientProjection::AccountSelection {
                installation_id,
                candidates,
                active,
                frontier,
            } => SnapshotItem::AccountSelection {
                installation_id: id32(installation_id.as_bytes()),
                candidates: candidates
                    .iter()
                    .map(|account| id32(account.as_bytes()))
                    .collect(),
                active: active.map(|account| id32(account.as_bytes())),
                frontier: frontier.iter().map(|fact| id32(fact.as_bytes())).collect(),
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
            ClientProjection::ProjectResource {
                project_id,
                resource_id,
                display_locator,
                canonical_locator,
                health,
                primary,
                active_claim,
                conflicting_projects,
            } => SnapshotItem::ProjectResource {
                project_id: id32(project_id.as_bytes()),
                resource_id: id32(resource_id.as_bytes()),
                display_locator: locator_to_v1(&display_locator),
                canonical_locator: locator_to_v1(&canonical_locator),
                health: resource_health_to_v1(health),
                primary,
                active_claim,
                conflicting_projects: conflicting_projects
                    .iter()
                    .map(|project| id32(project.as_bytes()))
                    .collect(),
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
                account_id,
                project_id,
                target_home,
                expected_head,
                operation,
                body,
                issued_at,
                request_fact,
                stage,
            } => SnapshotItem::RemoteCommand {
                command_id: id32(command_id.as_bytes()),
                request_digest: id32(request_digest.as_bytes()),
                account_id: id32(account_id.as_bytes()),
                project_id: id32(project_id.as_bytes()),
                target_home: id32(target_home.as_bytes()),
                expected_head: id32(expected_head.as_bytes()),
                operation_provider: operation.provider().as_str().to_owned(),
                operation_session: operation.session().as_str().to_owned(),
                operation_id: id32(operation.operation().as_bytes()),
                body: body.as_str().to_owned(),
                issued_at_unix_millis: issued_at.as_unix_millis(),
                request_fact: id32(request_fact.as_bytes()),
                progress: Box::new(remote_progress_to_v1(&stage)),
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
        request.body.enabled,
    );
    Ok(effect_from_v1(request, body))
}

pub(crate) fn relay_status_to_v1(status: &RelayStatus) -> RelayStatusDto {
    let mut policies = status
        .policies
        .iter()
        .map(|policy| RelayPolicyStatusDto {
            endpoint: locator_to_v1(&policy.endpoint),
            access: match policy.access {
                RelayAccess::Read => RelayAccessDto::Read,
                RelayAccess::Write => RelayAccessDto::Write,
                RelayAccess::ReadWrite => RelayAccessDto::ReadWrite,
            },
            authentication: match policy.authentication {
                RelayAuthentication::Disabled => RelayAuthenticationDto::Disabled,
                RelayAuthentication::OnChallenge => RelayAuthenticationDto::OnChallenge,
                RelayAuthentication::Required => RelayAuthenticationDto::Required,
            },
            enabled: policy.enabled,
            generation: policy.generation,
        })
        .collect::<Vec<_>>();
    policies.sort_by(|left, right| left.endpoint.cmp(&right.endpoint));
    RelayStatusDto {
        policies,
        queued: u64::try_from(status.queued).unwrap_or(u64::MAX),
        prepared: u64::try_from(status.prepared).unwrap_or(u64::MAX),
        uncertain: u64::try_from(status.uncertain).unwrap_or(u64::MAX),
        rejected: u64::try_from(status.rejected).unwrap_or(u64::MAX),
        accepted: u64::try_from(status.accepted).unwrap_or(u64::MAX),
        staged: u64::try_from(status.staged).unwrap_or(u64::MAX),
        quarantined: u64::try_from(status.quarantined).unwrap_or(u64::MAX),
        truncated: status.truncated,
    }
}

pub(crate) fn state_health_to_v1(status: &StateHealth) -> StateHealthDto {
    StateHealthDto {
        revision: status.revision.value(),
        domains: status.domains.iter().map(domain_health_to_v1).collect(),
    }
}

pub(crate) fn state_repair_to_v1(report: &StateRepairReport) -> StateRepairReportDto {
    StateRepairReportDto {
        operation_id: Id32::new(*report.operation_id.as_bytes()),
        revision: report.revision.value(),
        domains: report.domains.iter().map(domain_health_to_v1).collect(),
    }
}

fn domain_health_to_v1(health: &DomainHealth) -> DomainHealthDto {
    DomainHealthDto {
        domain: match health.domain {
            HealthDomain::Authority => HealthDomainDto::Authority,
            HealthDomain::Conversation => HealthDomainDto::Conversation,
            HealthDomain::Agent => HealthDomainDto::Agent,
            HealthDomain::Project => HealthDomainDto::Project,
        },
        projected: health.projected,
        unresolved: health.unresolved,
        unauthorized: health.unauthorized,
        conflicted: health.conflicted,
        invalid: health.invalid,
        unsupported: health.unsupported,
        conflicts: health.conflicts,
    }
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
    let body = ResourceInspectionRequest {
        project_id: ProjectId::from_bytes(request.body.project_id.bytes()),
        resource_id: ResourceId::from_bytes(request.body.resource_id.bytes()),
        display_locator: locator_from_v1(request.body.display_locator.clone())?,
        canonical_locator: locator_from_v1(request.body.canonical_locator.clone())?,
    };
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
        health: resource_health_to_v1(result.health),
        observed_canonical: result.observed_canonical.as_ref().map(locator_to_v1),
        details: result
            .details
            .as_ref()
            .map(|details| details.as_str().to_owned()),
        checked_at_unix_millis: result.checked_at.as_unix_millis(),
    })
}

/// Converts one strict local API request into the transport-independent command value.
pub fn project_command_from_v1(
    request: ProjectCommandRequestDto,
) -> Result<ProjectCommandRequest, ValueError> {
    let provisioning = matches!(
        &request.action,
        ProjectCommandActionDto::ProvisionWorktree(_)
    );
    if provisioning == request.expected_head.is_some() {
        return Err(ValueError::InvalidValueCombination);
    }
    Ok(ProjectCommandRequest {
        command_id: CommandId::from_bytes(request.command_id.bytes()),
        operation_id: OperationId::from_bytes(request.operation_id.bytes()),
        request_digest: CommandDigest::from_bytes(request.request_digest.bytes()),
        account_id: hq_domain::AccountId::from_bytes(request.account_id.bytes()),
        project_id: ProjectId::from_bytes(request.project_id.bytes()),
        home: hq_domain::InstallationId::from_bytes(request.home.bytes()),
        expected_head: request
            .expected_head
            .map(|head| hq_domain::FactId::from_bytes(head.bytes())),
        issued_at: Timestamp::from_unix_millis(request.issued_at_unix_millis),
        action: project_action_from_v1(request.action)?,
    })
}

/// Converts one typed project result into its local API representation.
pub fn project_command_to_v1(outcome: &ProjectCommandOutcome) -> ProjectCommandOutcomeDto {
    match outcome {
        ProjectCommandOutcome::Accepted {
            operation_id,
            stage,
        } => ProjectCommandOutcomeDto::Accepted {
            operation_id: id32(operation_id.as_bytes()),
            stage: project_stage_to_v1(*stage),
        },
        ProjectCommandOutcome::Running {
            operation_id,
            stage,
        } => ProjectCommandOutcomeDto::Running {
            operation_id: id32(operation_id.as_bytes()),
            stage: project_stage_to_v1(*stage),
        },
        ProjectCommandOutcome::Completed {
            operation_id,
            project_head,
            runtime,
        } => ProjectCommandOutcomeDto::Completed {
            operation_id: id32(operation_id.as_bytes()),
            project_head: id32(project_head.as_bytes()),
            runtime: runtime.as_ref().map(runtime_to_v1),
        },
        ProjectCommandOutcome::Rejected {
            operation_id,
            error,
            runtime,
        } => ProjectCommandOutcomeDto::Rejected {
            operation_id: id32(operation_id.as_bytes()),
            error: domain_error_to_v1(error),
            runtime: runtime.as_ref().map(runtime_to_v1),
        },
        ProjectCommandOutcome::Reconcilable {
            operation_id,
            stage,
            error,
        } => ProjectCommandOutcomeDto::Reconcilable {
            operation_id: id32(operation_id.as_bytes()),
            stage: project_stage_to_v1(*stage),
            error: domain_error_to_v1(error),
        },
    }
}

#[allow(clippy::too_many_lines)]
fn project_action_from_v1(
    action: ProjectCommandActionDto,
) -> Result<ProjectCommandAction, ValueError> {
    Ok(match action {
        ProjectCommandActionDto::Open => ProjectCommandAction::Open,
        ProjectCommandActionDto::Activate {
            agent_id,
            provider,
            resume_session,
            resume_thread,
            launch_directory,
        } => ProjectCommandAction::Activate {
            agent_id: AgentId::from_bytes(agent_id.bytes()),
            provider: ProviderId::new(provider).map_err(|_| ValueError::InvalidText)?,
            resume_session: resume_session
                .map(ProviderSessionId::new)
                .transpose()
                .map_err(|_| ValueError::InvalidText)?,
            resume_thread: resume_thread.map(|value| ThreadId::from_bytes(value.bytes())),
            launch_directory: locator_from_v1(launch_directory)?,
        },
        ProjectCommandActionDto::DispatchPending => ProjectCommandAction::DispatchPending,
        ProjectCommandActionDto::Close { force } => ProjectCommandAction::Close { force },
        ProjectCommandActionDto::SetArchived { archived } => {
            ProjectCommandAction::SetArchived { archived }
        }
        ProjectCommandActionDto::Handoff {
            agent_id,
            provider,
            resume_session,
            thread_id,
            launch_directory,
            force_takeover,
        } => ProjectCommandAction::Handoff {
            agent_id: AgentId::from_bytes(agent_id.bytes()),
            provider: ProviderId::new(provider).map_err(|_| ValueError::InvalidText)?,
            resume_session: resume_session
                .map(ProviderSessionId::new)
                .transpose()
                .map_err(|_| ValueError::InvalidText)?,
            thread_id: ThreadId::from_bytes(thread_id.bytes()),
            launch_directory: locator_from_v1(launch_directory)?,
            force_takeover,
        },
        ProjectCommandActionDto::RetireAgent { agent_id, force } => {
            ProjectCommandAction::RetireAgent {
                agent_id: AgentId::from_bytes(agent_id.bytes()),
                force,
            }
        }
        ProjectCommandActionDto::AddResource {
            resource,
            make_primary,
        } => ProjectCommandAction::AddResource {
            resource: project_resource_from_v1(resource)?,
            make_primary,
        },
        ProjectCommandActionDto::RemoveResource { resource_id, force } => {
            ProjectCommandAction::RemoveResource {
                resource_id: ResourceId::from_bytes(resource_id.bytes()),
                force,
            }
        }
        ProjectCommandActionDto::ReplaceResource {
            old_resource_id,
            new_resource,
        } => ProjectCommandAction::ReplaceResource {
            old_resource_id: ResourceId::from_bytes(old_resource_id.bytes()),
            new_resource: project_resource_from_v1(new_resource)?,
        },
        ProjectCommandActionDto::ProvisionWorktree(request) => {
            ProjectCommandAction::ProvisionWorktree(WorktreeProvisioningRequest {
                mailbox_id: MailboxId::from_bytes(request.mailbox_id.bytes()),
                project_name: ShortText::new(request.project_name)
                    .map_err(|_| ValueError::InvalidText)?,
                brief: request
                    .brief
                    .map(hq_domain::ContentText::new)
                    .transpose()
                    .map_err(|_| ValueError::InvalidText)?,
                source: locator_from_v1(request.source)?,
                destination: locator_from_v1(request.destination)?,
                branch: ShortText::new(request.branch).map_err(|_| ValueError::InvalidText)?,
                create_branch: request.create_branch,
            })
        }
    })
}

fn project_resource_from_v1(resource: ProjectResourceDto) -> Result<ProjectResource, ValueError> {
    Ok(ProjectResource {
        resource_id: ResourceId::from_bytes(resource.resource_id.bytes()),
        display_locator: locator_from_v1(resource.display_locator)?,
        canonical_locator: locator_from_v1(resource.canonical_locator)?,
        health: resource_health_from_v1(resource.health),
    })
}

const fn resource_health_from_v1(health: ResourceHealthDto) -> ResourceHealth {
    match health {
        ResourceHealthDto::Unknown => ResourceHealth::Unknown,
        ResourceHealthDto::Healthy => ResourceHealth::Healthy,
        ResourceHealthDto::Degraded => ResourceHealth::Degraded,
        ResourceHealthDto::Unavailable => ResourceHealth::Unavailable,
    }
}

fn runtime_to_v1(runtime: &RuntimeObservation) -> RuntimeObservationDto {
    match runtime {
        RuntimeObservation::Succeeded => RuntimeObservationDto::Succeeded,
        RuntimeObservation::Failed(code) => RuntimeObservationDto::Failed(code.as_str().to_owned()),
        RuntimeObservation::Uncertain(code) => {
            RuntimeObservationDto::Uncertain(code.as_str().to_owned())
        }
    }
}

fn remote_progress_to_v1(stage: &ClientRemoteCommandStage) -> RemoteCommandProgressDto {
    match stage {
        ClientRemoteCommandStage::Queued => RemoteCommandProgressDto::Queued,
        ClientRemoteCommandStage::Received {
            receipt_fact,
            received_head,
            received_at,
        } => RemoteCommandProgressDto::Received {
            receipt_fact: id32(receipt_fact.as_bytes()),
            received_head: id32(received_head.as_bytes()),
            received_at_unix_millis: received_at.as_unix_millis(),
        },
        ClientRemoteCommandStage::Terminal {
            receipt_fact,
            received_head,
            received_at,
            outcome_fact,
            result,
            runtime,
        } => RemoteCommandProgressDto::Terminal {
            receipt_fact: id32(receipt_fact.as_bytes()),
            received_head: id32(received_head.as_bytes()),
            received_at_unix_millis: received_at.as_unix_millis(),
            outcome_fact: id32(outcome_fact.as_bytes()),
            result: match result {
                RemoteCommandResult::Committed(head) => {
                    RemoteCommandResultDto::Committed(id32(head.as_bytes()))
                }
                RemoteCommandResult::Rejected(code) => {
                    RemoteCommandResultDto::Rejected(code.as_str().to_owned())
                }
            },
            runtime: runtime.as_ref().map(runtime_to_v1),
        },
        ClientRemoteCommandStage::Conflicted => RemoteCommandProgressDto::Conflicted,
    }
}

const fn project_stage_to_v1(stage: ProjectCommandStage) -> ProjectCommandStageDto {
    match stage {
        ProjectCommandStage::Accepted => ProjectCommandStageDto::Accepted,
        ProjectCommandStage::AwaitingHome => ProjectCommandStageDto::AwaitingHome,
        ProjectCommandStage::ReceivedAtHome => ProjectCommandStageDto::ReceivedAtHome,
        ProjectCommandStage::ValidatingResources => ProjectCommandStageDto::ValidatingResources,
        ProjectCommandStage::Opening => ProjectCommandStageDto::Opening,
        ProjectCommandStage::ConfiguringAssignment => ProjectCommandStageDto::ConfiguringAssignment,
        ProjectCommandStage::StartingRuntime => ProjectCommandStageDto::StartingRuntime,
        ProjectCommandStage::ValidatingLaunchDirectory => {
            ProjectCommandStageDto::ValidatingLaunchDirectory
        }
        ProjectCommandStage::MakingRunnable => ProjectCommandStageDto::MakingRunnable,
        ProjectCommandStage::DispatchingInputs => ProjectCommandStageDto::DispatchingInputs,
        ProjectCommandStage::AssessingRelease => ProjectCommandStageDto::AssessingRelease,
        ProjectCommandStage::QuiescingRuntime => ProjectCommandStageDto::QuiescingRuntime,
        ProjectCommandStage::EndingAssignment => ProjectCommandStageDto::EndingAssignment,
        ProjectCommandStage::Closing => ProjectCommandStageDto::Closing,
        ProjectCommandStage::UpdatingProject => ProjectCommandStageDto::UpdatingProject,
        ProjectCommandStage::ReservingDestination => ProjectCommandStageDto::ReservingDestination,
        ProjectCommandStage::ReconcilingGit => ProjectCommandStageDto::ReconcilingGit,
        ProjectCommandStage::CreatingWorktree => ProjectCommandStageDto::CreatingWorktree,
        ProjectCommandStage::IdentifyingResource => ProjectCommandStageDto::IdentifyingResource,
        ProjectCommandStage::CreatingProject => ProjectCommandStageDto::CreatingProject,
        ProjectCommandStage::Compensating => ProjectCommandStageDto::Compensating,
        ProjectCommandStage::ReconciliationRequired => {
            ProjectCommandStageDto::ReconciliationRequired
        }
        ProjectCommandStage::Complete => ProjectCommandStageDto::Complete,
    }
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

fn locator_to_v1(locator: &ResourceLocator) -> ResourceLocatorDto {
    ResourceLocatorDto {
        scheme: match locator.scheme() {
            ResourceScheme::GitRepository => ResourceSchemeDto::GitRepository,
            ResourceScheme::WorkingTree => ResourceSchemeDto::WorkingTree,
            ResourceScheme::Container => ResourceSchemeDto::Container,
            ResourceScheme::Opaque => ResourceSchemeDto::Opaque,
        },
        value: locator.value().to_owned(),
    }
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
