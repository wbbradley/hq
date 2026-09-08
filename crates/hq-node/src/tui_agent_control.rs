//! Independently owned local client for exact agent controls.

use std::{collections::BTreeMap, num::NonZeroU64};

use hq_local_api::{
    ClientEvent,
    protocol::v1::{
        AgentCancellationRequestDto, AgentCancellationStateDto, AgentOperationProjectDto,
        AgentOperationQueryDto, AgentOperationScopeDto, AgentOperationTargetDto,
        AgentOperationViewDto, AuthoritativeSnapshotDto, Id32, ResponseResult, SnapshotItem,
    },
};
use hq_tui::{
    UiAgentCancellationIntent, UiAgentCancellationOutcome, UiAgentOperationProject,
    UiAgentOperationQuery, UiAgentOperationScope, UiAgentOperationStatus, UiAgentOperationTarget,
    UiAgentOperationView, UiConversationId, UiFailure,
};

use super::{
    LocalNodeClient, SharedTuiPresentation, conversation_id_from_wire, conversation_id_to_wire,
    random_identity, tui_activity_status,
};

pub(super) fn control_accounts(
    snapshot: &AuthoritativeSnapshotDto,
) -> BTreeMap<[u8; 32], [u8; 32]> {
    snapshot
        .items
        .iter()
        .filter_map(|item| match item {
            SnapshotItem::AccountSelection {
                installation_id,
                active: Some(account),
                ..
            } if super::tui_human_state(installation_id.bytes(), snapshot)
                == hq_tui::UiHumanState::Ready =>
            {
                Some((installation_id.bytes(), account.bytes()))
            }
            _ => None,
        })
        .collect()
}

pub(super) enum AgentControlCommand {
    Query {
        id: hq_tui::EffectId,
        conversation: UiConversationId,
        tracked_operation: Option<[u8; 32]>,
    },
    Cancel {
        id: hq_tui::EffectId,
        intent: Box<UiAgentCancellationIntent>,
        observe: bool,
    },
}

pub(super) fn worker(
    mut port: Option<Box<dyn TuiAgentControlPort>>,
    commands: &std::sync::mpsc::Receiver<AgentControlCommand>,
    events: &std::sync::mpsc::SyncSender<hq_tui::UiEvent>,
    cancellation: &std::sync::atomic::AtomicBool,
    notifier: &super::TuiEventNotifier,
) {
    use hq_tui::UiEvent;
    use std::sync::{atomic::Ordering, mpsc::RecvTimeoutError};
    while !cancellation.load(Ordering::SeqCst) {
        let command = match commands.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if cancellation.load(Ordering::SeqCst) {
            break;
        }
        let event = match command {
            AgentControlCommand::Query {
                id,
                conversation,
                tracked_operation,
            } => {
                let result = port.as_mut().map_or_else(
                    || Ok(UiAgentOperationView::default()),
                    |port| port.query(conversation, tracked_operation),
                );
                UiEvent::AgentOperationLoaded {
                    effect_id: id,
                    result,
                }
            }
            AgentControlCommand::Cancel {
                id,
                intent,
                observe,
            } => {
                let outcome = port
                    .as_mut()
                    .map_or(UiAgentCancellationOutcome::Rejected, |port| {
                        port.cancel(&intent, observe)
                    });
                UiEvent::AgentCancellationCompleted {
                    effect_id: id,
                    intent_id: intent.id,
                    outcome,
                }
            }
        };
        if !super::send_tui_event(events, notifier, event) {
            break;
        }
    }
}

/// Owned independently of ordinary composer, project, and history commands.
pub trait TuiAgentControlPort: Send {
    /// Reads current authorized capability and canonical status for the exact conversation.
    fn query(
        &mut self,
        conversation: UiConversationId,
        tracked_operation: Option<[u8; 32]>,
    ) -> Result<UiAgentOperationView, UiFailure>;

    /// Submits or observes an immutable intent. Observation must never resubmit it.
    fn cancel(
        &mut self,
        intent: &UiAgentCancellationIntent,
        observe: bool,
    ) -> UiAgentCancellationOutcome;
}

pub(super) struct LocalTuiAgentControl {
    pub(super) client: LocalNodeClient,
    pub(super) presentation: SharedTuiPresentation,
    pub(super) requests: BTreeMap<NonZeroU64, AgentCancellationRequestDto>,
}

impl TuiAgentControlPort for LocalTuiAgentControl {
    fn query(
        &mut self,
        conversation: UiConversationId,
        tracked_operation: Option<[u8; 32]>,
    ) -> Result<UiAgentOperationView, UiFailure> {
        if matches!(conversation, UiConversationId::Thread { .. }) {
            return Ok(UiAgentOperationView::default());
        }
        let home = *self.client.installation_id().as_bytes();
        let account = self
            .presentation
            .inner
            .lock()
            .map_err(|_| unavailable())?
            .control_accounts
            .get(&home)
            .copied();
        let Some(account) = account else {
            return Ok(UiAgentOperationView::default());
        };
        let query = AgentOperationQueryDto {
            account_id: Id32::new(account),
            home: Id32::new(home),
            conversation: conversation_id_to_wire(&conversation),
            tracked_operation: tracked_operation.map(Id32::new),
        };
        let event = self
            .client
            .agent_operation(query)
            .map_err(|_| unavailable())?;
        match event {
            ClientEvent::Response {
                result: ResponseResult::AgentOperation(view),
                ..
            } => decode_view(*view),
            _ => Err(unavailable()),
        }
    }

    fn cancel(
        &mut self,
        intent: &UiAgentCancellationIntent,
        observe: bool,
    ) -> UiAgentCancellationOutcome {
        let request = if observe {
            let Some(request) = self.requests.get(&intent.id) else {
                return UiAgentCancellationOutcome::Uncertain;
            };
            if request.target != encode_target(&intent.target) {
                return UiAgentCancellationOutcome::Rejected;
            }
            request.clone()
        } else {
            match retain_request(&mut self.requests, intent, random_identity) {
                Ok(request) => request,
                Err(_) => return UiAgentCancellationOutcome::Rejected,
            }
        };
        let response = if observe {
            self.client.agent_cancellation_state(request)
        } else {
            self.client.cancel_agent_operation(request)
        };
        match response {
            Ok(ClientEvent::Response {
                result: ResponseResult::AgentCancellationState(state),
                ..
            }) => match state {
                AgentCancellationStateDto::Queued => UiAgentCancellationOutcome::Queued,
                AgentCancellationStateDto::Requested => UiAgentCancellationOutcome::Requested,
                AgentCancellationStateDto::AlreadyFinished => {
                    UiAgentCancellationOutcome::AlreadyFinished
                }
                AgentCancellationStateDto::Rejected { .. } => UiAgentCancellationOutcome::Rejected,
                AgentCancellationStateDto::Uncertain { .. } => {
                    UiAgentCancellationOutcome::Uncertain
                }
            },
            _ => UiAgentCancellationOutcome::Uncertain,
        }
    }
}

fn unavailable() -> UiFailure {
    UiFailure {
        code: "agent_control_unavailable".to_owned(),
        action: "reconnect and check the agent's status".to_owned(),
    }
}

fn retain_request(
    requests: &mut BTreeMap<NonZeroU64, AgentCancellationRequestDto>,
    intent: &UiAgentCancellationIntent,
    create_id: impl FnOnce() -> Result<[u8; 32], UiFailure>,
) -> Result<AgentCancellationRequestDto, UiFailure> {
    let target = encode_target(&intent.target);
    if let Some(request) = requests.get(&intent.id) {
        return if request.target == target {
            Ok(request.clone())
        } else {
            Err(unavailable())
        };
    }
    if requests.len() >= 1024 {
        return Err(unavailable());
    }
    let request = AgentCancellationRequestDto {
        request_id: Id32::new(create_id()?),
        target,
    };
    requests.insert(intent.id, request.clone());
    Ok(request)
}

fn encode_target(target: &UiAgentOperationTarget) -> AgentOperationTargetDto {
    AgentOperationTargetDto {
        query: AgentOperationQueryDto {
            account_id: Id32::new(target.query.account_id),
            home: Id32::new(target.query.home),
            conversation: conversation_id_to_wire(&target.query.conversation),
            tracked_operation: target.query.tracked_operation.map(Id32::new),
        },
        scope: AgentOperationScopeDto {
            agent_id: Id32::new(target.scope.agent_id),
            mailbox_installation: Id32::new(target.scope.mailbox_installation),
            mailbox_id: Id32::new(target.scope.mailbox_id),
            provider: target.scope.provider.clone(),
            session: target.scope.session.clone(),
            project: target
                .scope
                .project
                .as_ref()
                .map(|project| AgentOperationProjectDto {
                    project_id: Id32::new(project.project_id),
                    assignment_id: Id32::new(project.assignment_id),
                    thread_id: Id32::new(project.thread_id),
                }),
        },
        generation: Id32::new(target.generation),
        owner: Id32::new(target.owner),
        operation_id: Id32::new(target.operation_id),
        sequence: target.sequence.get(),
    }
}

fn decode_view(view: AgentOperationViewDto) -> Result<UiAgentOperationView, UiFailure> {
    hq_local_api::agent_operation_view_from_v1(view.clone()).map_err(|_| unavailable())?;
    Ok(UiAgentOperationView {
        target: view
            .target
            .map(|target| {
                Ok(UiAgentOperationTarget {
                    query: UiAgentOperationQuery {
                        account_id: target.query.account_id.bytes(),
                        home: target.query.home.bytes(),
                        conversation: conversation_id_from_wire(&target.query.conversation),
                        tracked_operation: target.query.tracked_operation.map(Id32::bytes),
                    },
                    scope: UiAgentOperationScope {
                        agent_id: target.scope.agent_id.bytes(),
                        mailbox_installation: target.scope.mailbox_installation.bytes(),
                        mailbox_id: target.scope.mailbox_id.bytes(),
                        provider: target.scope.provider,
                        session: target.scope.session,
                        project: target.scope.project.map(|project| UiAgentOperationProject {
                            project_id: project.project_id.bytes(),
                            assignment_id: project.assignment_id.bytes(),
                            thread_id: project.thread_id.bytes(),
                        }),
                    },
                    generation: target.generation.bytes(),
                    owner: target.owner.bytes(),
                    operation_id: target.operation_id.bytes(),
                    sequence: NonZeroU64::new(target.sequence).ok_or_else(unavailable)?,
                })
            })
            .transpose()?,
        tracked: view
            .tracked
            .map(|status| {
                Ok(UiAgentOperationStatus {
                    operation_id: status.operation_id.bytes(),
                    sequence: NonZeroU64::new(status.sequence).ok_or_else(unavailable)?,
                    status: tui_activity_status(status.status),
                })
            })
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]
    use super::*;
    use hq_local_api::protocol::v1::ConversationKeyDto;

    fn target(project: bool) -> AgentOperationTargetDto {
        AgentOperationTargetDto {
            query: AgentOperationQueryDto {
                account_id: Id32::new([1; 32]),
                home: Id32::new([2; 32]),
                conversation: if project {
                    ConversationKeyDto::ProjectThread {
                        project: Id32::new([3; 32]),
                        thread: Id32::new([4; 32]),
                    }
                } else {
                    ConversationKeyDto::ProviderSession {
                        counterparty_installation: Id32::new([2; 32]),
                        counterparty_mailbox: Id32::new([5; 32]),
                        provider: "provider".to_owned(),
                        session: "session".to_owned(),
                    }
                },
                tracked_operation: Some(Id32::new([6; 32])),
            },
            scope: AgentOperationScopeDto {
                agent_id: Id32::new([7; 32]),
                mailbox_installation: Id32::new([2; 32]),
                mailbox_id: Id32::new([5; 32]),
                provider: "provider".to_owned(),
                session: "session".to_owned(),
                project: project.then_some(AgentOperationProjectDto {
                    project_id: Id32::new([3; 32]),
                    assignment_id: Id32::new([8; 32]),
                    thread_id: Id32::new([4; 32]),
                }),
            },
            generation: Id32::new([9; 32]),
            owner: Id32::new([10; 32]),
            operation_id: Id32::new([11; 32]),
            sequence: 12,
        }
    }

    #[test]
    fn tui_target_roundtrip_preserves_all_authority_and_replay_evidence() {
        for project in [false, true] {
            let wire = target(project);
            let ui = decode_view(AgentOperationViewDto {
                target: Some(wire.clone()),
                tracked: None,
            })
            .expect("valid target")
            .target
            .expect("target");
            assert_eq!(encode_target(&ui), wire);
            let intent = UiAgentCancellationIntent {
                id: NonZeroU64::MIN,
                target: ui,
            };
            let mut requests = BTreeMap::new();
            let first =
                retain_request(&mut requests, &intent, || Ok([13; 32])).expect("first request");
            let replay = retain_request(&mut requests, &intent, || {
                panic!("replay cannot create a new identity")
            })
            .expect("same request");
            assert_eq!(replay, first);
            let mut changed = intent.clone();
            changed.target.query.tracked_operation = None;
            assert!(
                retain_request(&mut requests, &changed, || panic!(
                    "changed replay cannot create identity"
                ))
                .is_err()
            );
            assert_eq!(requests.get(&intent.id), Some(&first));
        }
    }

    #[test]
    fn invalid_control_evidence_never_becomes_a_ui_capability() {
        let mut wire = target(false);
        wire.sequence = 0;
        assert!(
            decode_view(AgentOperationViewDto {
                target: Some(wire),
                tracked: None
            })
            .is_err()
        );
        let mut wire = target(false);
        wire.generation = Id32::new([0; 32]);
        assert!(
            decode_view(AgentOperationViewDto {
                target: Some(wire),
                tracked: None
            })
            .is_err()
        );
    }
}
