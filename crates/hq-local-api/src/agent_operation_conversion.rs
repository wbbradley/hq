//! Exact provider-neutral cancellation values across the local wire boundary.

use super::{
    activity_status_to_v1, conversation_key_from_v1, conversation_key_to_v1, id32,
    runtime_failure_reason_to_v1,
};
use crate::protocol::v1::{
    ActivityStatusDto, AgentCancellationRequestDto, AgentCancellationStateDto,
    AgentOperationProjectDto, AgentOperationQueryDto, AgentOperationScopeDto,
    AgentOperationStatusDto, AgentOperationTargetDto, AgentOperationViewDto,
    RuntimeFailureReasonDto, ValueError,
};
use hq_application as app;
use hq_domain::{
    AccountId, AgentId, AssignmentId, InstallationId, MailboxAddress, MailboxId, OperationId,
    ProjectId, ProviderId, ProviderSessionId, ThreadId,
};
use std::num::NonZeroU64;

/// Encodes the exact actor and selected conversation.
pub fn agent_operation_query_to_v1(query: &app::AgentOperationQuery) -> AgentOperationQueryDto {
    AgentOperationQueryDto {
        account_id: id32(query.account_id.as_bytes()),
        home: id32(query.home.as_bytes()),
        conversation: conversation_key_to_v1(&query.conversation),
        tracked_operation: query.tracked_operation.map(|id| id32(id.as_bytes())),
    }
}

/// Decodes validated recipient/session selectors without inferring authority.
pub fn agent_operation_query_from_v1(
    query: AgentOperationQueryDto,
) -> Result<app::AgentOperationQuery, ValueError> {
    Ok(app::AgentOperationQuery {
        account_id: AccountId::from_bytes(query.account_id.bytes()),
        home: InstallationId::from_bytes(query.home.bytes()),
        conversation: conversation_key_from_v1(query.conversation)?,
        tracked_operation: query
            .tracked_operation
            .map(|id| OperationId::from_bytes(id.bytes())),
    })
}

fn target_to_v1(target: &app::AgentOperationTarget) -> AgentOperationTargetDto {
    AgentOperationTargetDto {
        query: agent_operation_query_to_v1(&target.query),
        scope: AgentOperationScopeDto {
            agent_id: id32(target.scope.agent_id.as_bytes()),
            mailbox_installation: id32(target.scope.mailbox.installation_id().as_bytes()),
            mailbox_id: id32(target.scope.mailbox.mailbox_id().as_bytes()),
            provider: target.scope.provider.as_str().to_owned(),
            session: target.scope.session.as_str().to_owned(),
            project: target
                .scope
                .project
                .as_ref()
                .map(|project| AgentOperationProjectDto {
                    project_id: id32(project.project_id.as_bytes()),
                    assignment_id: id32(project.assignment_id.as_bytes()),
                    thread_id: id32(project.thread_id.as_bytes()),
                }),
        },
        generation: id32(target.generation.as_bytes()),
        owner: id32(target.owner.as_bytes()),
        operation_id: id32(target.operation_id.as_bytes()),
        sequence: target.sequence.get(),
    }
}

fn target_from_v1(
    target: AgentOperationTargetDto,
) -> Result<app::AgentOperationTarget, ValueError> {
    Ok(app::AgentOperationTarget {
        query: agent_operation_query_from_v1(target.query)?,
        scope: app::AgentOperationScope {
            agent_id: AgentId::from_bytes(target.scope.agent_id.bytes()),
            mailbox: MailboxAddress::new(
                InstallationId::from_bytes(target.scope.mailbox_installation.bytes()),
                MailboxId::from_bytes(target.scope.mailbox_id.bytes()),
            ),
            provider: ProviderId::new(target.scope.provider)
                .map_err(|_| ValueError::InvalidText)?,
            session: ProviderSessionId::new(target.scope.session)
                .map_err(|_| ValueError::InvalidText)?,
            project: target
                .scope
                .project
                .map(|project| app::AgentOperationProject {
                    project_id: ProjectId::from_bytes(project.project_id.bytes()),
                    assignment_id: AssignmentId::from_bytes(project.assignment_id.bytes()),
                    thread_id: ThreadId::from_bytes(project.thread_id.bytes()),
                }),
        },
        generation: app::RuntimeGenerationId::from_bytes(target.generation.bytes())
            .ok_or(ValueError::InvalidValueCombination)?,
        owner: app::RuntimeWorkerOwner::from_bytes(target.owner.bytes())
            .ok_or(ValueError::InvalidValueCombination)?,
        operation_id: OperationId::from_bytes(target.operation_id.bytes()),
        sequence: NonZeroU64::new(target.sequence).ok_or(ValueError::InvalidValueCombination)?,
    })
}

/// Encodes a stable request for submission or observation.
pub fn agent_cancellation_request_to_v1(
    request: &app::AgentCancellationRequest,
) -> AgentCancellationRequestDto {
    AgentCancellationRequestDto {
        request_id: id32(request.request_id.as_bytes()),
        target: target_to_v1(&request.target),
    }
}

/// Decodes an exact request, rejecting absent runtime identities.
pub fn agent_cancellation_request_from_v1(
    request: AgentCancellationRequestDto,
) -> Result<app::AgentCancellationRequest, ValueError> {
    Ok(app::AgentCancellationRequest {
        request_id: OperationId::from_bytes(request.request_id.bytes()),
        target: target_from_v1(request.target)?,
    })
}

/// Encodes live capability separately from authoritative turn status.
pub fn agent_operation_view_to_v1(view: &app::AgentOperationView) -> AgentOperationViewDto {
    AgentOperationViewDto {
        target: view.target.as_ref().map(target_to_v1),
        tracked: view.tracked.as_ref().map(|status| AgentOperationStatusDto {
            operation_id: id32(status.operation_id.as_bytes()),
            sequence: status.sequence.get(),
            status: activity_status_to_v1(&status.status),
        }),
    }
}

/// Decodes current capability and exact canonical status.
pub fn agent_operation_view_from_v1(
    view: AgentOperationViewDto,
) -> Result<app::AgentOperationView, ValueError> {
    Ok(app::AgentOperationView {
        target: view.target.map(target_from_v1).transpose()?,
        tracked: view
            .tracked
            .map(|status| {
                Ok::<_, ValueError>(app::AgentOperationStatus {
                    operation_id: OperationId::from_bytes(status.operation_id.bytes()),
                    sequence: NonZeroU64::new(status.sequence)
                        .ok_or(ValueError::InvalidValueCombination)?,
                    status: match status.status {
                        ActivityStatusDto::Snapshot => hq_domain::ActivityStatus::Snapshot,
                        ActivityStatusDto::Running => hq_domain::ActivityStatus::Running,
                        ActivityStatusDto::Succeeded => hq_domain::ActivityStatus::Succeeded,
                        ActivityStatusDto::Interrupted => hq_domain::ActivityStatus::Interrupted,
                        ActivityStatusDto::Failed { reason } => hq_domain::ActivityStatus::Failed(
                            hq_domain::ErrorCode::new(reason)
                                .map_err(|_| ValueError::InvalidText)?,
                        ),
                    },
                })
            })
            .transpose()?,
    })
}

/// Encodes request progress without claiming the turn has stopped.
pub fn agent_cancellation_state_to_v1(
    state: &app::AgentCancellationState,
) -> AgentCancellationStateDto {
    match state {
        app::AgentCancellationState::Queued => AgentCancellationStateDto::Queued,
        app::AgentCancellationState::Requested => AgentCancellationStateDto::Requested,
        app::AgentCancellationState::AlreadyFinished => AgentCancellationStateDto::AlreadyFinished,
        app::AgentCancellationState::Rejected(reason) => AgentCancellationStateDto::Rejected {
            reason: runtime_failure_reason_to_v1(*reason),
        },
        app::AgentCancellationState::Uncertain(reason) => AgentCancellationStateDto::Uncertain {
            reason: runtime_failure_reason_to_v1(*reason),
        },
    }
}

/// Decodes bounded request progress independently of canonical work state.
pub fn agent_cancellation_state_from_v1(
    state: AgentCancellationStateDto,
) -> app::AgentCancellationState {
    match state {
        AgentCancellationStateDto::Queued => app::AgentCancellationState::Queued,
        AgentCancellationStateDto::Requested => app::AgentCancellationState::Requested,
        AgentCancellationStateDto::AlreadyFinished => app::AgentCancellationState::AlreadyFinished,
        AgentCancellationStateDto::Rejected { reason } => {
            app::AgentCancellationState::Rejected(failure_from_v1(reason))
        }
        AgentCancellationStateDto::Uncertain { reason } => {
            app::AgentCancellationState::Uncertain(failure_from_v1(reason))
        }
    }
}

const fn failure_from_v1(reason: RuntimeFailureReasonDto) -> app::RuntimeFailureReason {
    match reason {
        RuntimeFailureReasonDto::GenerationChanged => app::RuntimeFailureReason::GenerationChanged,
        RuntimeFailureReasonDto::InvalidInput => app::RuntimeFailureReason::InvalidInput,
        RuntimeFailureReasonDto::Unsupported => app::RuntimeFailureReason::Unsupported,
        RuntimeFailureReasonDto::ProviderNotRegistered => {
            app::RuntimeFailureReason::ProviderNotRegistered
        }
        RuntimeFailureReasonDto::RegistrationConflict => {
            app::RuntimeFailureReason::RegistrationConflict
        }
        RuntimeFailureReasonDto::UnsafeRecovery => app::RuntimeFailureReason::UnsafeRecovery,
        RuntimeFailureReasonDto::SessionIdentityMismatch => {
            app::RuntimeFailureReason::SessionIdentityMismatch
        }
        RuntimeFailureReasonDto::SessionNotFound => app::RuntimeFailureReason::SessionNotFound,
        RuntimeFailureReasonDto::SubmissionIdentityConflict => {
            app::RuntimeFailureReason::SubmissionIdentityConflict
        }
        RuntimeFailureReasonDto::InteractiveAlreadyAnswered => {
            app::RuntimeFailureReason::InteractiveAlreadyAnswered
        }
        RuntimeFailureReasonDto::SecretInputRejected => {
            app::RuntimeFailureReason::SecretInputRejected
        }
        RuntimeFailureReasonDto::IntakeClosed => app::RuntimeFailureReason::IntakeClosed,
        RuntimeFailureReasonDto::Crashed => app::RuntimeFailureReason::Crashed,
        RuntimeFailureReasonDto::ProtocolViolation => app::RuntimeFailureReason::ProtocolViolation,
        RuntimeFailureReasonDto::TransportClosed => app::RuntimeFailureReason::TransportClosed,
        RuntimeFailureReasonDto::ProcessFailed => app::RuntimeFailureReason::ProcessFailed,
        RuntimeFailureReasonDto::CompatibilityMismatch => {
            app::RuntimeFailureReason::CompatibilityMismatch
        }
        RuntimeFailureReasonDto::Unavailable => app::RuntimeFailureReason::Unavailable,
        RuntimeFailureReasonDto::CleanupFailed => app::RuntimeFailureReason::CleanupFailed,
        RuntimeFailureReasonDto::OwnershipConflict => app::RuntimeFailureReason::OwnershipConflict,
        RuntimeFailureReasonDto::Backpressure => app::RuntimeFailureReason::Backpressure,
        RuntimeFailureReasonDto::PersistenceCollision => {
            app::RuntimeFailureReason::PersistenceCollision
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use hq_application as app;
    use hq_domain::*;
    use std::num::NonZeroU64;

    fn request() -> app::AgentCancellationRequest {
        let home = InstallationId::from_bytes([1; 32]);
        let mailbox = MailboxAddress::new(home, MailboxId::from_bytes([2; 32]));
        let provider = ProviderId::new("worker").expect("provider");
        let session = ProviderSessionId::new("session").expect("session");
        app::AgentCancellationRequest {
            request_id: OperationId::from_bytes([3; 32]),
            target: app::AgentOperationTarget {
                query: app::AgentOperationQuery {
                    account_id: AccountId::from_bytes([4; 32]),
                    home,
                    conversation: app::ConversationKey::ProviderSession {
                        counterparty: mailbox,
                        provider: provider.clone(),
                        session: session.clone(),
                    },
                    tracked_operation: Some(OperationId::from_bytes([5; 32])),
                },
                scope: app::AgentOperationScope {
                    agent_id: AgentId::from_bytes([6; 32]),
                    mailbox,
                    provider,
                    session,
                    project: None,
                },
                generation: app::RuntimeGenerationId::from_bytes([7; 32]).expect("generation"),
                owner: app::RuntimeWorkerOwner::from_bytes([8; 32]).expect("owner"),
                operation_id: OperationId::from_bytes([9; 32]),
                sequence: NonZeroU64::MIN,
            },
        }
    }

    #[test]
    fn cancellation_target_round_trip_preserves_every_identity() {
        let expected = request();
        let wire = agent_cancellation_request_to_v1(&expected);
        assert_eq!(
            agent_cancellation_request_from_v1(wire).expect("decode"),
            expected
        );
    }

    #[test]
    fn framed_requests_validate_and_preserve_project_assignment_targets() {
        let mut expected = request();
        let project = ProjectId::from_bytes([10; 32]);
        let thread = ThreadId::from_bytes([11; 32]);
        expected.target.query.conversation = app::ConversationKey::ProjectThread {
            project_id: project,
            thread,
        };
        expected.target.scope.project = Some(app::AgentOperationProject {
            project_id: project,
            assignment_id: AssignmentId::from_bytes([12; 32]),
            thread_id: thread,
        });
        let wire = agent_cancellation_request_to_v1(&expected);
        assert_eq!(
            agent_cancellation_request_from_v1(wire.clone()).expect("project target"),
            expected
        );
        for request in [
            crate::protocol::v1::Request::CancelAgentOperation(Box::new(wire.clone())),
            crate::protocol::v1::Request::AgentCancellationState(Box::new(wire.clone())),
        ] {
            let message = crate::protocol::v1::WireMessage::Request(
                crate::protocol::v1::RequestEnvelope::new(
                    crate::protocol::v1::RequestId::new(1).expect("id"),
                    request,
                ),
            );
            let frame = message.encode_frame().expect("frame");
            assert_eq!(
                crate::protocol::v1::WireMessage::decode_frame(&frame).expect("decode"),
                message
            );
        }
        let mut invalid = wire;
        invalid.target.sequence = 0;
        let message =
            crate::protocol::v1::WireMessage::Request(crate::protocol::v1::RequestEnvelope::new(
                crate::protocol::v1::RequestId::new(1).expect("id"),
                crate::protocol::v1::Request::CancelAgentOperation(Box::new(invalid)),
            ));
        assert!(
            message.encode_frame().is_err(),
            "invalid target cannot enter the wire"
        );
    }

    #[test]
    fn cancellation_rejects_invalid_runtime_identity_and_sequence() {
        let wire = agent_cancellation_request_to_v1(&request());
        let mut zero = wire.clone();
        zero.target.sequence = 0;
        assert!(agent_cancellation_request_from_v1(zero).is_err());
        let mut zero = wire.clone();
        zero.target.owner = crate::protocol::v1::Id32::new([0; 32]);
        assert!(agent_cancellation_request_from_v1(zero).is_err());
        let mut zero = wire;
        zero.target.generation = crate::protocol::v1::Id32::new([0; 32]);
        assert!(agent_cancellation_request_from_v1(zero).is_err());
    }

    #[test]
    fn acknowledgement_and_authoritative_terminal_evidence_remain_distinct() {
        let request = request();
        let view = app::AgentOperationView {
            target: None,
            tracked: Some(app::AgentOperationStatus {
                operation_id: request.target.operation_id,
                sequence: NonZeroU64::MIN,
                status: ActivityStatus::Interrupted,
            }),
        };
        assert_eq!(
            agent_operation_view_from_v1(agent_operation_view_to_v1(&view)).expect("view"),
            view
        );
        for state in [
            app::AgentCancellationState::Queued,
            app::AgentCancellationState::Requested,
            app::AgentCancellationState::AlreadyFinished,
            app::AgentCancellationState::Rejected(app::RuntimeFailureReason::OwnershipConflict),
            app::AgentCancellationState::Uncertain(app::RuntimeFailureReason::Unavailable),
        ] {
            assert_eq!(
                agent_cancellation_state_from_v1(agent_cancellation_state_to_v1(&state)),
                state
            );
        }
    }
}
