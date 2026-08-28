//! Pure installation-local named-agent and session-metadata planning.

use std::collections::BTreeSet;

use hq_domain::{
    AgentId, AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, FactId, FactScope,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, MailboxAddress, MailboxId, MailboxKind, ProviderId,
    ProviderSessionId, RepositoryContext, SemanticPayload, ShortText,
};

use crate::{
    ApplicationError, ApplicationErrorCode, FactPlan, LocalFactInputs, LocalInstallationAuthority,
};

/// Passive complete intent for one permanent installation-local agent-name claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentNameClaimRequest {
    /// Stable named-agent identity.
    pub agent_id: AgentId,
    /// Exact installation-qualified agent mailbox.
    pub mailbox: MailboxAddress,
    /// Exact projected agent-mailbox creation fact.
    pub mailbox_root: FactId,
    /// Permanent lowercase installation-local name.
    pub name: ShortText,
}

/// Passive complete evidence for one installation-local absorbing agent retirement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRetirementPlanRequest {
    /// Stable named-agent identity.
    pub agent_id: AgentId,
    /// Exact installation-qualified agent mailbox.
    pub mailbox: MailboxAddress,
    /// Exact compatible permanent name claim.
    pub claim_fact: FactId,
    /// Complete causal-maximal agent/session lifecycle frontier observed before retirement.
    pub agent_frontier: BTreeSet<FactId>,
}

/// Passive complete intent for one exact durable provider-session selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionSelectionRequest {
    /// Stable named-agent identity.
    pub agent_id: AgentId,
    /// Exact installation-qualified agent mailbox.
    pub mailbox: MailboxAddress,
    /// Exact compatible permanent name claim.
    pub claim_fact: FactId,
    /// Neutral provider namespace.
    pub provider: ProviderId,
    /// Exact immutable provider session.
    pub session: ProviderSessionId,
    /// Exact compatible mailbox/session binding.
    pub binding_fact: FactId,
    /// Exact matching repository-context fact.
    pub context_fact: FactId,
    /// Exact typed repository context carried by that fact.
    pub context: RepositoryContext,
    /// Complete causal-maximal selection register being replaced.
    pub selection_frontier: BTreeSet<FactId>,
}

/// Passive complete intent for one immutable mailbox/provider-session binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionBindingRequest {
    /// Exact installation-qualified agent mailbox.
    pub mailbox: MailboxAddress,
    /// Exact projected agent-mailbox creation fact.
    pub mailbox_root: FactId,
    /// Neutral provider namespace.
    pub provider: ProviderId,
    /// Exact immutable provider session acknowledged ready.
    pub session: ProviderSessionId,
}

/// Passive complete intent for one mailbox repository-context record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionContextRequest {
    /// Exact installation-qualified agent mailbox.
    pub mailbox: MailboxAddress,
    /// Exact projected agent-mailbox creation fact.
    pub mailbox_root: FactId,
    /// Validated repository and launch-directory context.
    pub context: RepositoryContext,
}

/// Passive complete intent for one exact provider-session display rename or clear.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionRenameRequest {
    /// Stable named-agent identity.
    pub agent_id: AgentId,
    /// Exact installation-qualified agent mailbox.
    pub mailbox: MailboxAddress,
    /// Exact compatible permanent name claim.
    pub claim_fact: FactId,
    /// Neutral provider namespace.
    pub provider: ProviderId,
    /// Exact immutable provider session.
    pub session: ProviderSessionId,
    /// Exact compatible mailbox/session binding.
    pub binding_fact: FactId,
    /// Replacement display name, or `None` for an explicit clear.
    pub display_name: Option<ShortText>,
    /// Complete causal-maximal rename register being replaced.
    pub rename_frontier: BTreeSet<FactId>,
}

/// Plans creation of one installation-local agent mailbox.
pub fn plan_agent_mailbox_creation(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    mailbox_id: MailboxId,
    label: Option<ShortText>,
) -> Result<FactPlan, ApplicationError> {
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, BTreeSet::new())?,
        SemanticPayload::MailboxCreated {
            mailbox_id,
            kind: MailboxKind::Agent,
            label,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one permanent installation-local named-agent claim.
pub fn plan_agent_name_claim(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: AgentNameClaimRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id
        || !valid_agent_name(request.name.as_str())
    {
        return Err(invalid_request());
    }
    let mut parents = BTreeSet::from([request.mailbox_root]);
    let causal = local_causal(authority, std::mem::take(&mut parents))?;
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        causal,
        SemanticPayload::AgentNameClaimed {
            agent_id: request.agent_id,
            mailbox_id: request.mailbox.mailbox_id(),
            name: request.name,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one frontier-complete absorbing installation-local agent retirement.
pub fn plan_agent_retirement(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: AgentRetirementPlanRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id {
        return Err(invalid_request());
    }
    let mut parents = request.agent_frontier;
    parents.insert(request.claim_fact);
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, parents)?,
        SemanticPayload::AgentRetired {
            agent_id: request.agent_id,
            mailbox_id: request.mailbox.mailbox_id(),
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one immutable provider-session binding after exact runtime readiness.
pub fn plan_agent_session_binding(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: AgentSessionBindingRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id {
        return Err(invalid_request());
    }
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, BTreeSet::from([request.mailbox_root]))?,
        SemanticPayload::MailboxSessionBound {
            mailbox_id: request.mailbox.mailbox_id(),
            provider: request.provider,
            session: request.session,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one immutable repository context after validating the launch directory.
pub fn plan_agent_session_context(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: AgentSessionContextRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id {
        return Err(invalid_request());
    }
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, BTreeSet::from([request.mailbox_root]))?,
        SemanticPayload::MailboxContextRecorded {
            mailbox_id: request.mailbox.mailbox_id(),
            context: request.context,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one frontier-complete exact durable provider-session selection.
pub fn plan_agent_session_selection(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: AgentSessionSelectionRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id {
        return Err(invalid_request());
    }
    let mut parents = request.selection_frontier;
    parents.extend([
        request.claim_fact,
        request.binding_fact,
        request.context_fact,
    ]);
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, parents)?,
        SemanticPayload::ProviderSessionSelected {
            agent_id: request.agent_id,
            mailbox_id: request.mailbox.mailbox_id(),
            provider: request.provider,
            session: request.session,
            context: request.context,
        },
        inputs.auxiliary_randomness,
    ))
}

/// Plans one frontier-complete exact provider-session display rename or clear.
pub fn plan_agent_session_rename(
    authority: LocalInstallationAuthority,
    inputs: LocalFactInputs,
    request: AgentSessionRenameRequest,
) -> Result<FactPlan, ApplicationError> {
    if request.mailbox.installation_id() != authority.installation_id {
        return Err(invalid_request());
    }
    let mut parents = request.rename_frontier;
    parents.extend([request.claim_fact, request.binding_fact]);
    Ok(FactPlan::new(
        authority.installation_id,
        inputs.authored_at,
        FactScope::InstallationPrivate(authority.installation_id),
        local_causal(authority, parents)?,
        SemanticPayload::ProviderSessionRenamed {
            agent_id: request.agent_id,
            provider: request.provider,
            session: request.session,
            display_name: request.display_name,
        },
        inputs.auxiliary_randomness,
    ))
}

fn local_causal(
    authority: LocalInstallationAuthority,
    mut parents: BTreeSet<FactId>,
) -> Result<CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>, ApplicationError> {
    parents.insert(authority.root_fact);
    CausalReferences::new(
        BoundedSet::new(parents).map_err(|_| invalid_request())?,
        [AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            authority.root_fact,
        )],
    )
    .map_err(|_| invalid_request())
}

fn valid_agent_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

const fn invalid_request() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}
