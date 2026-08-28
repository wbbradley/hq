//! Pure canonical planning for provider-neutral harness output and activity.

use std::{collections::BTreeSet, num::NonZeroU64};

use hq_domain::{
    ActivityKind, ActivityStatus, AuthorityReference, AuthorityRole, BoundedSet, CausalReferences,
    ContentText, FactId, FactScope, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS,
    MailboxAddress, MessageContent, MessageId, MessagePurpose, OperationCorrelation,
    PresentationKind, SemanticPayload, ShortText, Timestamp,
};

use crate::{ApplicationError, ApplicationErrorCode, FactPlan, LocalFactInputs};

/// Exact local authority and source binding for canonical harness values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessAuthoringAuthority {
    /// Installation signing the canonical fact.
    pub author: InstallationId,
    /// Exact active agent mailbox represented as the source.
    pub source: MailboxAddress,
    /// Reserved local human mailbox receiving user-facing output.
    pub recipient: MailboxAddress,
    /// Exact local-installation authority reference.
    pub authority: AuthorityReference,
    /// Installation, mailbox, session, and optional prior-value support.
    pub support: BTreeSet<FactId>,
}

/// Passive normalized user-facing output intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessOutputFactRequest {
    /// Stable public output identity.
    pub output_id: MessageId,
    /// Exact provider/session/operation correlation.
    pub correlation: OperationCorrelation,
    /// Typed message presentation.
    pub presentation: PresentationKind,
    /// Bounded user-facing body.
    pub body: ContentText,
}

/// Passive normalized non-actionable activity intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessActivityFactRequest {
    /// Exact provider/session/operation correlation.
    pub correlation: OperationCorrelation,
    /// Optional provider item identity.
    pub item: Option<ShortText>,
    /// Reducer-owned activity class.
    pub kind: ActivityKind,
    /// Stable logical key within the operation.
    pub logical_key: ShortText,
    /// Bounded runtime lifetime identity.
    pub runtime: ShortText,
    /// Positive semantic source sequence.
    pub sequence: NonZeroU64,
    /// Explicit event occurrence time.
    pub occurred_at: Timestamp,
    /// Typed activity state.
    pub status: ActivityStatus,
    /// Bounded user-facing content.
    pub content: ContentText,
    /// Whether provider content was explicitly shortened.
    pub truncated: bool,
}

/// Plans one correlated agent output message under exact local binding evidence.
pub fn plan_harness_output(
    authority: &HarnessAuthoringAuthority,
    inputs: LocalFactInputs,
    request: HarnessOutputFactRequest,
) -> Result<FactPlan, ApplicationError> {
    validate(authority)?;
    let causal = causal(authority)?;
    let content = MessageContent {
        message_id: request.output_id,
        sender: authority.source,
        recipient: Some(authority.recipient),
        body: request.body,
        purpose: MessagePurpose::Asynchronous,
        presentation: request.presentation,
        correlation: Some(request.correlation),
        project_id: None,
    };
    Ok(FactPlan::new(
        authority.author,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.author),
        causal,
        SemanticPayload::AsynchronousMessageSent(content),
        inputs.auxiliary_randomness,
    ))
}

/// Plans one correlated non-actionable activity record under exact local binding evidence.
pub fn plan_harness_activity(
    authority: &HarnessAuthoringAuthority,
    inputs: LocalFactInputs,
    request: HarnessActivityFactRequest,
) -> Result<FactPlan, ApplicationError> {
    validate(authority)?;
    let causal = causal(authority)?;
    Ok(FactPlan::new(
        authority.author,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.author),
        causal,
        SemanticPayload::HarnessActivityRecorded {
            source: authority.source,
            correlation: request.correlation,
            item: request.item,
            kind: request.kind,
            logical_key: request.logical_key,
            runtime: request.runtime,
            sequence: request.sequence,
            occurred_at: request.occurred_at,
            status: request.status,
            content: request.content,
            truncated: request.truncated,
        },
        inputs.auxiliary_randomness,
    ))
}

fn validate(authority: &HarnessAuthoringAuthority) -> Result<(), ApplicationError> {
    (authority.source.installation_id() == authority.author
        && authority.recipient.installation_id() == authority.author
        && authority.source != authority.recipient
        && authority.authority.role() == AuthorityRole::LocalInstallation)
        .then_some(())
        .ok_or_else(invalid)
}

fn causal(
    authority: &HarnessAuthoringAuthority,
) -> Result<CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>, ApplicationError> {
    let parents = BoundedSet::new(authority.support.clone()).map_err(|_| invalid())?;
    CausalReferences::new(parents, [authority.authority]).map_err(|_| invalid())
}

const fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use hq_domain::{MailboxId, OperationId, ProviderId, ProviderSessionId};

    use super::*;

    fn authority() -> HarnessAuthoringAuthority {
        let author = InstallationId::from_bytes([1; 32]);
        HarnessAuthoringAuthority {
            author,
            source: MailboxAddress::new(author, MailboxId::from_bytes([2; 32])),
            recipient: MailboxAddress::new(author, MailboxId::from_bytes([3; 32])),
            authority: AuthorityReference::new(
                AuthorityRole::LocalInstallation,
                FactId::from_bytes([4; 32]),
            ),
            support: BTreeSet::from([FactId::from_bytes([4; 32]), FactId::from_bytes([5; 32])]),
        }
    }

    fn inputs() -> LocalFactInputs {
        LocalFactInputs {
            authored_at: Timestamp::from_unix_millis(10),
            auxiliary_randomness: [11; 32],
        }
    }

    fn correlation() -> OperationCorrelation {
        OperationCorrelation::new(
            ProviderId::new("provider").expect("provider"),
            ProviderSessionId::new("session").expect("session"),
            OperationId::from_bytes([6; 32]),
        )
    }

    #[test]
    fn output_is_a_correlated_agent_message_with_complete_support() {
        let plan = plan_harness_output(
            &authority(),
            inputs(),
            HarnessOutputFactRequest {
                output_id: MessageId::from_bytes([7; 32]),
                correlation: correlation(),
                presentation: PresentationKind::FinalAnswer,
                body: ContentText::new("done").expect("body"),
            },
        )
        .expect("output plan");
        let SemanticPayload::AsynchronousMessageSent(message) = plan.payload() else {
            panic!("expected output message");
        };
        assert_eq!(message.sender, authority().source);
        assert_eq!(message.recipient, Some(authority().recipient));
        assert_eq!(message.correlation.as_ref(), Some(&correlation()));
        assert_eq!(message.presentation, PresentationKind::FinalAnswer);
        assert_eq!(
            plan.causal().parents().iter().count(),
            authority().support.len()
        );
        assert!(
            authority()
                .support
                .iter()
                .all(|fact| plan.causal().parents().contains(fact))
        );
        assert_eq!(
            plan.causal().authority(AuthorityRole::LocalInstallation),
            Some(authority().authority.fact_id())
        );
    }

    #[test]
    fn activity_preserves_every_normalized_field_and_rejects_cross_installation_sources() {
        let request = HarnessActivityFactRequest {
            correlation: correlation(),
            item: Some(ShortText::new("item").expect("item")),
            kind: ActivityKind::Progress,
            logical_key: ShortText::new("plan").expect("key"),
            runtime: ShortText::new("runtime").expect("runtime"),
            sequence: NonZeroU64::new(2).expect("sequence"),
            occurred_at: Timestamp::from_unix_millis(12),
            status: ActivityStatus::Running,
            content: ContentText::new("working").expect("content"),
            truncated: true,
        };
        let plan =
            plan_harness_activity(&authority(), inputs(), request.clone()).expect("activity plan");
        assert!(matches!(
            plan.payload(),
            SemanticPayload::HarnessActivityRecorded {
                correlation: value,
                sequence,
                truncated: true,
                ..
            } if value == &request.correlation && *sequence == request.sequence
        ));

        let mut invalid_authority = authority();
        invalid_authority.source = MailboxAddress::new(
            InstallationId::from_bytes([9; 32]),
            MailboxId::from_bytes([2; 32]),
        );
        let error = plan_harness_activity(&invalid_authority, inputs(), request)
            .expect_err("cross-installation source rejects");
        assert_eq!(error.code(), ApplicationErrorCode::InvalidRequest);
    }
}
