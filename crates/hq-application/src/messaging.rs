//! Pure message, reply, cancellation, and reversible-state fact planning.

use std::collections::BTreeSet;

use hq_domain::{
    AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, ContentText, FactId,
    FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxAddress,
    MessageContent, MessageId, MessagePurpose, PresentationKind, ProjectId, SemanticPayload,
    ThreadId,
};

use crate::{ApplicationError, ApplicationErrorCode, FactPlan, LocalFactInputs};

/// Exact historical authority and semantic sender for one authored message action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageAuthoringAuthority {
    /// Installation signing the fact.
    pub author: InstallationId,
    /// Mailbox represented as the message sender.
    pub sender: MailboxAddress,
    /// Audience and routing scope.
    pub scope: FactScope,
    /// Exact historical authority supporting the action.
    pub authority: AuthorityReference,
    /// Required mailbox, installation, membership, or grant support facts.
    pub support: BTreeSet<FactId>,
}

/// Complete passive intent for one new question or asynchronous root message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewMessageRequest {
    /// Stable public message identity.
    pub message_id: MessageId,
    /// Optional direct recipient; account-addressed messages omit it.
    pub recipient: Option<MailboxAddress>,
    /// Bounded message body.
    pub body: ContentText,
    /// Typed presentation behavior.
    pub presentation: PresentationKind,
    /// Optional project association.
    pub project_id: Option<ProjectId>,
}

/// Complete passive intent for one asynchronous project-thread continuation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinueProjectMessageRequest {
    /// Existing stable project exchange.
    pub thread_id: ThreadId,
    /// Exact initiating message fact.
    pub root_fact: FactId,
    /// Immutable initiating message used to validate project addressing.
    pub root: MessageContent,
    /// Immutable initiating audience.
    pub root_scope: FactScope,
    /// Stable public continuation identity.
    pub message_id: MessageId,
    /// Bounded message body.
    pub body: ContentText,
    /// Typed presentation behavior.
    pub presentation: PresentationKind,
}

/// Complete passive intent for one causal answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyRequest {
    /// Stable thread derived from the root fact.
    pub thread_id: ThreadId,
    /// Exact question root fact.
    pub root_fact: FactId,
    /// Immutable root content used to validate reversed addressing.
    pub root: MessageContent,
    /// Immutable root audience used to prevent cross-scope replies.
    pub root_scope: FactScope,
    /// Stable public answer identity.
    pub message_id: MessageId,
    /// Bounded answer body.
    pub body: ContentText,
    /// Typed answer presentation.
    pub presentation: PresentationKind,
}

/// Complete passive intent for cancelling one question thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadCancellationRequest {
    /// Stable thread derived from the root fact.
    pub thread_id: ThreadId,
    /// Exact question root fact.
    pub root_fact: FactId,
    /// Immutable root content proving cancellation ownership.
    pub root: MessageContent,
    /// Immutable root audience used to prevent cross-scope cancellation.
    pub root_scope: FactScope,
    /// Optional bounded human-readable cancellation reason.
    pub reason: Option<ContentText>,
}

/// Complete passive intent for one reversible local message-state action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageStateRequest {
    /// Target public message identity.
    pub message_id: MessageId,
    /// Exact target message-bearing fact.
    pub target_fact: FactId,
    /// Complete causal-maximal state frontier.
    pub state_frontier: BTreeSet<FactId>,
}

/// Plans one question root.
pub fn plan_question(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: NewMessageRequest,
) -> Result<FactPlan, ApplicationError> {
    plan_root(authority, inputs, request, MessagePurpose::Question)
}

/// Plans one asynchronous root.
pub fn plan_asynchronous_message(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: NewMessageRequest,
) -> Result<FactPlan, ApplicationError> {
    plan_root(authority, inputs, request, MessagePurpose::Asynchronous)
}

/// Plans one causally bound continuation of an asynchronous project exchange.
pub fn plan_project_message_continuation(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: ContinueProjectMessageRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.root_scope != authority.scope
        || request.root.purpose != MessagePurpose::Asynchronous
        || request.thread_id != ThreadId::from_bytes(*request.root_fact.as_bytes())
        || request.root.sender != authority.sender
        || request.root.recipient.is_none()
        || request.root.project_id.is_none()
    {
        return Err(invalid());
    }
    validate_authority_with_project(&authority, request.root.recipient, true)?;
    let content = MessageContent {
        message_id: request.message_id,
        sender: authority.sender,
        recipient: request.root.recipient,
        body: request.body,
        purpose: MessagePurpose::Asynchronous,
        presentation: request.presentation,
        correlation: None,
        project_id: request.root.project_id,
    };
    let causal = causal(&authority, [request.root_fact])?;
    Ok(FactPlan::new(
        authority.author,
        inputs.authored_at,
        authority.scope,
        causal,
        SemanticPayload::AsynchronousMessageSent {
            thread_id: Some(request.thread_id),
            message: content,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one causally and directionally valid answer.
pub fn plan_reply(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: ReplyRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.root_scope != authority.scope
        || request.root.purpose != MessagePurpose::Question
        || request.thread_id != ThreadId::from_bytes(*request.root_fact.as_bytes())
        || request.root.sender == authority.sender
        || match request.root.recipient {
            Some(recipient) => recipient != authority.sender,
            None => !matches!(authority.scope, FactScope::AccountAddressed(_)),
        }
    {
        return Err(invalid());
    }
    let recipient = match authority.scope {
        FactScope::AccountAddressed(_) => None,
        FactScope::InstallationPrivate(_) | FactScope::PeerAddressed(_) => {
            Some(request.root.sender)
        }
        FactScope::RemoteControl { .. } => return Err(invalid()),
    };
    validate_authority(&authority, recipient)?;
    let content = MessageContent {
        message_id: request.message_id,
        sender: authority.sender,
        recipient,
        body: request.body,
        purpose: MessagePurpose::Question,
        presentation: request.presentation,
        correlation: request.root.correlation,
        project_id: request.root.project_id,
    };
    let causal = causal(&authority, [request.root_fact])?;
    Ok(FactPlan::new(
        authority.author,
        inputs.authored_at,
        authority.scope,
        causal,
        SemanticPayload::AnswerGiven {
            thread_id: request.thread_id,
            message: content,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans cancellation by the mailbox that authored the question root.
pub fn plan_thread_cancellation(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: ThreadCancellationRequest,
) -> Result<FactPlan, ApplicationError> {
    validate_authority(&authority, request.root.recipient)?;
    if request.root_scope != authority.scope
        || request.root.purpose != MessagePurpose::Question
        || request.root.sender != authority.sender
        || request.thread_id != ThreadId::from_bytes(*request.root_fact.as_bytes())
    {
        return Err(invalid());
    }
    let causal = causal(&authority, [request.root_fact])?;
    Ok(FactPlan::new(
        authority.author,
        inputs.authored_at,
        authority.scope,
        causal,
        SemanticPayload::ThreadCancelled {
            thread_id: request.thread_id,
            reason: request.reason,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one reversible archive action that causally descends from the target and state frontier.
pub fn plan_message_archive(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: MessageStateRequest,
) -> Result<FactPlan, ApplicationError> {
    plan_message_state(authority, inputs, request, false)
}

/// Plans one reversible restore action that causally descends from the target and state frontier.
pub fn plan_message_restore(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: MessageStateRequest,
) -> Result<FactPlan, ApplicationError> {
    plan_message_state(authority, inputs, request, true)
}

fn plan_root(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: NewMessageRequest,
    purpose: MessagePurpose,
) -> Result<FactPlan, ApplicationError> {
    validate_root_authority(&authority, request.recipient, request.project_id)?;
    let causal = causal(&authority, [])?;
    let content = MessageContent {
        message_id: request.message_id,
        sender: authority.sender,
        recipient: request.recipient,
        body: request.body,
        purpose,
        presentation: request.presentation,
        correlation: None,
        project_id: request.project_id,
    };
    Ok(FactPlan::new(
        authority.author,
        inputs.authored_at,
        authority.scope,
        causal,
        match purpose {
            MessagePurpose::Question => SemanticPayload::QuestionAsked(content),
            MessagePurpose::Asynchronous => SemanticPayload::AsynchronousMessageSent {
                thread_id: None,
                message: content,
            },
            MessagePurpose::ProjectOutput => return Err(invalid()),
        },
        inputs.auxiliary_randomness,
    ))
}

fn validate_root_authority(
    authority: &MessageAuthoringAuthority,
    recipient: Option<MailboxAddress>,
    project_id: Option<ProjectId>,
) -> Result<(), ApplicationError> {
    if matches!(authority.scope, FactScope::AccountAddressed(_))
        && (recipient.is_some() != project_id.is_some())
    {
        return Err(invalid());
    }
    validate_authority_with_project(authority, recipient, project_id.is_some())
}

fn plan_message_state(
    authority: MessageAuthoringAuthority,
    inputs: LocalFactInputs,
    request: MessageStateRequest,
    restore: bool,
) -> Result<FactPlan, ApplicationError> {
    validate_authority(&authority, None)?;
    let causal = causal(
        &authority,
        request
            .state_frontier
            .into_iter()
            .chain([request.target_fact]),
    )?;
    Ok(FactPlan::new(
        authority.author,
        inputs.authored_at,
        authority.scope,
        causal,
        if restore {
            SemanticPayload::MessageRestored {
                message_id: request.message_id,
            }
        } else {
            SemanticPayload::MessageArchived {
                message_id: request.message_id,
            }
        },
        inputs.auxiliary_randomness,
    ))
}

fn validate_authority(
    authority: &MessageAuthoringAuthority,
    recipient: Option<MailboxAddress>,
) -> Result<(), ApplicationError> {
    validate_authority_with_project(authority, recipient, false)
}

fn validate_authority_with_project(
    authority: &MessageAuthoringAuthority,
    recipient: Option<MailboxAddress>,
    project_addressed: bool,
) -> Result<(), ApplicationError> {
    if authority.sender.installation_id() != authority.author {
        return Err(invalid());
    }
    let valid = match (&authority.scope, authority.authority.role()) {
        (FactScope::InstallationPrivate(home), AuthorityRole::LocalInstallation) => {
            *home == authority.author
                && recipient.is_none_or(|recipient| recipient.installation_id() == *home)
        }
        (FactScope::PeerAddressed(target), AuthorityRole::MailboxGrant) => {
            recipient == Some(*target)
        }
        (FactScope::AccountAddressed(_), AuthorityRole::AccountMembership) => {
            recipient.is_none() || project_addressed
        }
        _ => false,
    };
    valid.then_some(()).ok_or_else(invalid)
}

fn causal(
    authority: &MessageAuthoringAuthority,
    extra_parents: impl IntoIterator<Item = FactId>,
) -> Result<CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>, ApplicationError> {
    let mut parents = authority.support.clone();
    parents.extend(extra_parents);
    let parents = BoundedSet::new(parents).map_err(|_| invalid())?;
    CausalReferences::new(parents, [authority.authority]).map_err(|_| invalid())
}

const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}
