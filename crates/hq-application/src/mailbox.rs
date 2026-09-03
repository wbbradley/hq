//! Durable local mailbox drafts and authoritative mailbox-command planning.

use std::collections::BTreeSet;

use hq_domain::{
    AgentId, AuthorityReference, AuthorityRole, CommandDigest, CommandId, ContentText, DomainError,
    ErrorCategory, ErrorCode, FactScope, InstallationId, MailboxAddress, MailboxKind, MessageId,
    OperationId, PresentationKind, ProjectId, ProviderId, ThreadId, Timestamp,
};
use hq_reducer::{
    AuthorityProjection, AuthorityProjectionKey, ConversationProjection, ConversationProjectionKey,
    MembershipState, ProjectProjection, ProjectProjectionKey,
};

use crate::{
    ContinueProjectMessageRequest, DomainSnapshot, LocalFactInputs, MessageAuthoringAuthority,
    MessageStateRequest, MutationDecision, NewMessageRequest, ReplyRequest,
    plan_asynchronous_message, plan_message_archive, plan_message_restore,
    plan_project_message_continuation, plan_reply,
};

/// Maximum installation-local drafts retained at once.
pub const MAX_MAILBOX_DRAFTS: usize = 128;

/// Explicit semantic target retained with a local draft.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailboxDraftTarget {
    /// Answer one authoritative question message.
    Reply { message_id: MessageId },
    /// Send one asynchronous message to an exact mailbox.
    Direct { recipient: MailboxAddress },
    /// Write an asynchronous note visible only to the local human mailbox.
    SelfNote,
    /// Send to a project's immutable mailbox, optionally continuing one exact exchange.
    Project {
        project_id: ProjectId,
        thread_id: Option<ThreadId>,
    },
    /// Prepare the first conversation with one exact named agent and provider.
    ProjectSetup {
        project_id: ProjectId,
        agent_id: AgentId,
        provider: ProviderId,
    },
}

/// Passive installation-local draft record shared by application and persistence boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxDraft {
    /// Stable draft identity.
    pub draft_id: OperationId,
    /// Explicit target retained even when its canonical referent becomes stale.
    pub target: MailboxDraftTarget,
    /// Possibly-empty UTF-8 composition text, bounded at the service boundary.
    pub content: String,
    /// Monotonic per-draft optimistic-concurrency version.
    pub version: u64,
}

/// Autosave input for one complete draft replacement.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxDraftSaveRequest {
    pub draft_id: OperationId,
    pub target: MailboxDraftTarget,
    pub content: String,
    /// Required current version, or `None` only when creating a new draft.
    pub expected_version: Option<u64>,
}

/// Result of one optimistic draft autosave.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailboxDraftSaveOutcome {
    Saved(MailboxDraft),
    Conflict(MailboxDraft),
}

/// Optimistic deletion input for one draft.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MailboxDraftDeleteRequest {
    pub draft_id: OperationId,
    pub expected_version: u64,
}

/// Result of one idempotent optimistic draft deletion.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailboxDraftDeleteOutcome {
    Deleted,
    NotFound,
    Conflict(MailboxDraft),
}

/// Authoritative mailbox mutation selected by a passive client.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailboxCommandAction {
    Reply {
        target_message: MessageId,
        message_id: MessageId,
    },
    Direct {
        recipient: MailboxAddress,
        message_id: MessageId,
    },
    SelfNote {
        message_id: MessageId,
    },
    Project {
        project_id: ProjectId,
        thread_id: Option<ThreadId>,
        message_id: MessageId,
    },
    Archive {
        target_message: MessageId,
    },
    Restore {
        target_message: MessageId,
    },
}

/// Stable retry envelope for one node-resolved mailbox command.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailboxCommandRequest {
    pub command_id: CommandId,
    pub request_digest: CommandDigest,
    /// Draft consumed only if this command commits successfully.
    pub draft_id: Option<OperationId>,
    pub action: MailboxCommandAction,
    /// Inline CLI content; draft-backed submissions leave this absent.
    pub content: Option<String>,
    pub authored_at: Timestamp,
    pub auxiliary_randomness: [u8; 32],
}

/// Plans one mailbox command exclusively from the transaction-consistent authoritative snapshot.
#[allow(clippy::too_many_lines)]
pub fn plan_mailbox_command(
    snapshot: &DomainSnapshot,
    local_installation: InstallationId,
    local_human: MailboxAddress,
    request: &MailboxCommandRequest,
    draft: Option<&MailboxDraft>,
) -> MutationDecision {
    let authority = match local_human_authority(snapshot, local_installation, local_human) {
        Ok(authority) => authority,
        Err(error) => return MutationDecision::reject(error),
    };
    let inputs = LocalFactInputs {
        authored_at: request.authored_at,
        auxiliary_randomness: request.auxiliary_randomness,
    };
    let planned = match &request.action {
        MailboxCommandAction::Reply {
            target_message,
            message_id,
        } => {
            let body = content(
                request,
                draft,
                &MailboxDraftTarget::Reply {
                    message_id: *target_message,
                },
            );
            body.and_then(|body| {
                let message = message(snapshot, *target_message)?;
                let thread = snapshot
                    .conversation()
                    .projection(ConversationProjectionKey::Thread(message.thread_id))
                    .and_then(|projection| match projection {
                        ConversationProjection::Thread(thread) => Some(thread),
                        _ => None,
                    })
                    .ok_or_else(stale_target)?;
                plan_reply(
                    authority,
                    inputs,
                    ReplyRequest {
                        thread_id: message.thread_id,
                        root_fact: thread.root_fact,
                        root: message.content.clone(),
                        root_scope: FactScope::InstallationPrivate(local_installation),
                        message_id: *message_id,
                        body,
                        presentation: PresentationKind::Message,
                    },
                )
                .map_err(|_| invalid_command())
            })
        }
        MailboxCommandAction::Direct {
            recipient,
            message_id,
        } => {
            let body = content(
                request,
                draft,
                &MailboxDraftTarget::Direct {
                    recipient: *recipient,
                },
            );
            body.and_then(|body| {
                if recipient.installation_id() != local_installation
                    || *recipient == local_human
                    || !matches!(
                        snapshot.authority().projection(AuthorityProjectionKey::Mailbox(*recipient)),
                        Some(AuthorityProjection::Mailbox(mailbox)) if mailbox.kind == MailboxKind::Agent
                    )
                {
                    return Err(stale_target());
                }
                plan_asynchronous_message(
                    authority,
                    inputs,
                    NewMessageRequest {
                        message_id: *message_id,
                        recipient: Some(*recipient),
                        body,
                        presentation: PresentationKind::Message,
                        project_id: None,
                    },
                )
                .map_err(|_| invalid_command())
            })
        }
        MailboxCommandAction::SelfNote { message_id } => {
            content(request, draft, &MailboxDraftTarget::SelfNote).and_then(|body| {
                plan_asynchronous_message(
                    authority,
                    inputs,
                    NewMessageRequest {
                        message_id: *message_id,
                        recipient: Some(local_human),
                        body,
                        presentation: PresentationKind::Message,
                        project_id: None,
                    },
                )
                .map_err(|_| invalid_command())
            })
        }
        MailboxCommandAction::Project {
            project_id,
            thread_id,
            message_id,
        } => {
            let target = match draft.map(|draft| &draft.target) {
                Some(
                    target @ MailboxDraftTarget::ProjectSetup {
                        project_id: target_project,
                        ..
                    },
                ) if target_project == project_id && thread_id.is_none() => target.clone(),
                _ => MailboxDraftTarget::Project {
                    project_id: *project_id,
                    thread_id: *thread_id,
                },
            };
            content(request, draft, &target).and_then(|body| {
                let (project, authority) =
                    project_authority(snapshot, local_installation, local_human, *project_id)?;
                if let Some(thread_id) = thread_id {
                    let thread = snapshot
                        .conversation()
                        .projection(ConversationProjectionKey::Thread(*thread_id))
                        .and_then(|projection| match projection {
                            ConversationProjection::Thread(thread) => Some(thread),
                            _ => None,
                        })
                        .ok_or_else(stale_target)?;
                    let root = snapshot
                        .conversation()
                        .projection(ConversationProjectionKey::Message(thread.root_message))
                        .and_then(|projection| match projection {
                            ConversationProjection::Message(message) => Some(message),
                            _ => None,
                        })
                        .ok_or_else(stale_target)?;
                    if root.fact_id != thread.root_fact
                        || root.thread_id != *thread_id
                        || root.content.project_id != Some(*project_id)
                        || root.content.recipient != Some(project.mailbox)
                    {
                        return Err(stale_target());
                    }
                    plan_project_message_continuation(
                        authority,
                        inputs,
                        ContinueProjectMessageRequest {
                            thread_id: *thread_id,
                            root_fact: root.fact_id,
                            root: root.content.clone(),
                            root_scope: FactScope::AccountAddressed(project.account_id),
                            message_id: *message_id,
                            body,
                            presentation: PresentationKind::Message,
                        },
                    )
                    .map_err(|_| invalid_command())
                } else {
                    plan_asynchronous_message(
                        authority,
                        inputs,
                        NewMessageRequest {
                            message_id: *message_id,
                            recipient: Some(project.mailbox),
                            body,
                            presentation: PresentationKind::Message,
                            project_id: Some(*project_id),
                        },
                    )
                    .map_err(|_| invalid_command())
                }
            })
        }
        MailboxCommandAction::Archive { target_message }
        | MailboxCommandAction::Restore { target_message } => {
            if request.draft_id.is_some() || request.content.is_some() || draft.is_some() {
                Err(invalid_command())
            } else {
                message(snapshot, *target_message).and_then(|message| {
                    let state = MessageStateRequest {
                        message_id: *target_message,
                        target_fact: message.fact_id,
                        state_frontier: message.state_frontier.clone(),
                    };
                    if matches!(request.action, MailboxCommandAction::Archive { .. }) {
                        plan_message_archive(authority, inputs, state)
                    } else {
                        plan_message_restore(authority, inputs, state)
                    }
                    .map_err(|_| invalid_command())
                })
            }
        }
    };
    planned.map_or_else(MutationDecision::reject, MutationDecision::commit)
}

fn content(
    request: &MailboxCommandRequest,
    draft: Option<&MailboxDraft>,
    expected_target: &MailboxDraftTarget,
) -> Result<ContentText, DomainError> {
    let value = match (request.draft_id, draft, request.content.as_deref()) {
        (Some(draft_id), Some(draft), None)
            if draft.draft_id == draft_id && &draft.target == expected_target =>
        {
            draft.content.as_str()
        }
        (None, None, Some(content)) => content,
        _ => return Err(invalid_command()),
    };
    ContentText::new(value).map_err(|_| invalid_command())
}

fn message(
    snapshot: &DomainSnapshot,
    message_id: MessageId,
) -> Result<&hq_reducer::MessageView, DomainError> {
    snapshot
        .conversation()
        .projection(ConversationProjectionKey::Message(message_id))
        .and_then(|projection| match projection {
            ConversationProjection::Message(message) => Some(message.as_ref()),
            _ => None,
        })
        .ok_or_else(stale_target)
}

fn local_human_authority(
    snapshot: &DomainSnapshot,
    local_installation: InstallationId,
    local_human: MailboxAddress,
) -> Result<MessageAuthoringAuthority, DomainError> {
    if local_human.installation_id() != local_installation {
        return Err(invalid_command());
    }
    let root_fact = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(local_installation))
        .and_then(|projection| match projection {
            AuthorityProjection::Installation(installation) => Some(installation.root_fact),
            _ => None,
        })
        .ok_or_else(stale_target)?;
    let mailbox_fact = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Mailbox(local_human))
        .and_then(|projection| match projection {
            AuthorityProjection::Mailbox(mailbox) if mailbox.kind == MailboxKind::Human => {
                Some(mailbox.create_fact)
            }
            _ => None,
        })
        .ok_or_else(stale_target)?;
    Ok(MessageAuthoringAuthority {
        author: local_installation,
        sender: local_human,
        scope: FactScope::InstallationPrivate(local_installation),
        authority: AuthorityReference::new(AuthorityRole::LocalInstallation, root_fact),
        support: BTreeSet::from([root_fact, mailbox_fact]),
    })
}

fn project_authority(
    snapshot: &DomainSnapshot,
    local_installation: InstallationId,
    local_human: MailboxAddress,
    project_id: ProjectId,
) -> Result<(&hq_reducer::ProjectView, MessageAuthoringAuthority), DomainError> {
    if local_human.installation_id() != local_installation {
        return Err(invalid_command());
    }
    let project = snapshot
        .project()
        .projection(ProjectProjectionKey::Project(project_id))
        .and_then(|projection| match projection {
            ProjectProjection::Project(project) => Some(project.as_ref()),
            _ => None,
        })
        .ok_or_else(stale_target)?;
    let selected = snapshot
        .authority()
        .projection(AuthorityProjectionKey::AccountSelection(local_installation))
        .and_then(|projection| match projection {
            AuthorityProjection::AccountSelection { active, .. } => *active,
            _ => None,
        });
    if selected != Some(project.account_id) {
        return Err(stale_target());
    }
    let membership_fact = match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Account(project.account_id))
    {
        Some(AuthorityProjection::Account {
            root_fact, creator, ..
        }) if creator.installation_id() == local_installation => *root_fact,
        _ => snapshot
            .authority()
            .projection(AuthorityProjectionKey::Membership {
                account: project.account_id,
                device: local_installation,
            })
            .and_then(|projection| match projection {
                AuthorityProjection::Membership(membership)
                    if membership.state() == MembershipState::Active =>
                {
                    membership.active_acceptances.iter().next().copied()
                }
                _ => None,
            })
            .ok_or_else(stale_target)?,
    };
    let mailbox_fact = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Mailbox(local_human))
        .and_then(|projection| match projection {
            AuthorityProjection::Mailbox(mailbox) if mailbox.kind == MailboxKind::Human => {
                Some(mailbox.create_fact)
            }
            _ => None,
        })
        .ok_or_else(stale_target)?;
    Ok((
        project,
        MessageAuthoringAuthority {
            author: local_installation,
            sender: local_human,
            scope: FactScope::AccountAddressed(project.account_id),
            authority: AuthorityReference::new(AuthorityRole::AccountMembership, membership_fact),
            support: BTreeSet::from([membership_fact, mailbox_fact]),
        },
    ))
}

fn invalid_command() -> DomainError {
    domain_error(ErrorCategory::InvalidInput, "invalid_mailbox_command")
}

fn stale_target() -> DomainError {
    domain_error(ErrorCategory::NotFound, "mailbox_target_stale")
}

fn domain_error(category: ErrorCategory, code: &str) -> DomainError {
    let Ok(code) = ErrorCode::new(code) else {
        unreachable!("static mailbox error code is bounded")
    };
    DomainError::new(category, code)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;

    use hq_domain::{
        AccountId, EncryptionPublicKey, FactId, InstallationAddress, MailboxId, ShortText,
        SigningPublicKey,
    };
    use hq_reducer::{
        AuthorityAggregateKey, AuthorityProjection, AuthorityProjectionKey, InstallationView,
        MailboxView, ProjectAggregateKey, ProjectLifecycle, ProjectProjection,
        ProjectProjectionKey, ProjectView,
    };

    use super::*;
    use crate::ProjectionSnapshot;

    #[test]
    fn project_messages_are_accepted_independent_of_runtime_readiness() {
        let cases = [
            (ProjectLifecycle::Open, false, true),
            (ProjectLifecycle::Closing, false, true),
            (ProjectLifecycle::Closed, false, true),
            (ProjectLifecycle::Closed, true, true),
            (ProjectLifecycle::Open, false, false),
        ];

        for (lifecycle, archived, claimable) in cases {
            let (snapshot, installation, human, project_id) =
                project_snapshot(lifecycle, archived, claimable);
            let decision = plan_mailbox_command(
                &snapshot,
                installation,
                human,
                &MailboxCommandRequest {
                    command_id: CommandId::from_bytes([11; 32]),
                    request_digest: CommandDigest::from_bytes([12; 32]),
                    draft_id: None,
                    action: MailboxCommandAction::Project {
                        project_id,
                        thread_id: None,
                        message_id: MessageId::from_bytes([13; 32]),
                    },
                    content: Some("queued follow-up".to_owned()),
                    authored_at: Timestamp::from_unix_millis(1),
                    auxiliary_randomness: [14; 32],
                },
                None,
            );

            assert!(
                matches!(decision, MutationDecision::Commit(_)),
                "project messaging unexpectedly followed runtime state: lifecycle={lifecycle:?}, archived={archived}, claimable={claimable}"
            );
        }
    }

    fn project_snapshot(
        lifecycle: ProjectLifecycle,
        archived: bool,
        claimable: bool,
    ) -> (DomainSnapshot, InstallationId, MailboxAddress, ProjectId) {
        let installation = InstallationId::from_bytes([1; 32]);
        let human = MailboxAddress::new(installation, MailboxId::from_bytes([2; 32]));
        let account = AccountId::from_bytes([3; 32]);
        let project_id = ProjectId::from_bytes([4; 32]);
        let account_fact = FactId::from_bytes([5; 32]);
        let mailbox_fact = FactId::from_bytes([6; 32]);
        let project_mailbox = MailboxAddress::new(installation, MailboxId::from_bytes([7; 32]));
        let installation_fact = FactId::from_bytes([8; 32]);
        let signing_key = SigningPublicKey::from_bytes([8; 32]);

        let authority = BTreeMap::from([
            (
                AuthorityProjectionKey::Installation(installation),
                AuthorityProjection::Installation(InstallationView {
                    root_fact: installation_fact,
                    signing_key,
                    encryption_key: EncryptionPublicKey::from_bytes([9; 32]),
                    label: None,
                }),
            ),
            (
                AuthorityProjectionKey::Account(account),
                AuthorityProjection::Account {
                    root_fact: account_fact,
                    creator: InstallationAddress::new(installation, signing_key),
                    label: None,
                },
            ),
            (
                AuthorityProjectionKey::AccountSelection(installation),
                AuthorityProjection::AccountSelection {
                    candidates: BTreeSet::from([account]),
                    active: Some(account),
                },
            ),
            (
                AuthorityProjectionKey::Mailbox(human),
                AuthorityProjection::Mailbox(MailboxView {
                    create_fact: mailbox_fact,
                    kind: MailboxKind::Human,
                    label: None,
                }),
            ),
        ]);
        let projects = BTreeMap::from([(
            ProjectProjectionKey::Project(project_id),
            ProjectProjection::Project(Box::new(ProjectView {
                root: FactId::from_bytes([9; 32]),
                head: FactId::from_bytes([10; 32]),
                fork_participants: BTreeSet::new(),
                home: installation,
                account_id: account,
                mailbox: project_mailbox,
                predecessor: None,
                name: ShortText::new("project").expect("bounded project name"),
                brief: None,
                resources: BTreeMap::new(),
                primary: None,
                lifecycle,
                archived,
                active_claims: BTreeSet::new(),
                claim_conflicts: BTreeMap::new(),
                claimable,
                assignment: None,
                input_sequence: 1,
            })),
        )]);
        let snapshot = DomainSnapshot::new(
            ProjectionSnapshot::new(
                BTreeMap::<AuthorityAggregateKey, _>::new(),
                authority,
                BTreeMap::new(),
            ),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ProjectionSnapshot::new(
                BTreeMap::<ProjectAggregateKey, _>::new(),
                projects,
                BTreeMap::new(),
            ),
        );
        (snapshot, installation, human, project_id)
    }
}
