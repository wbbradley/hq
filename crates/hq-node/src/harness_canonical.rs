//! Canonical named-agent session preparation and exact post-readiness selection.

use std::{collections::BTreeSet, path::PathBuf};

use hq_application::{
    AgentSessionBindingRequest, AgentSessionContextRequest, AgentSessionSelectionRequest,
    ApplicationError, ApplicationErrorCode, CommitFacts, FactMutation, LocalFactInputs,
    LocalInstallationAuthority, MutationAttempt, MutationDecision, MutationOutcome, QueryDomain,
    plan_agent_session_binding, plan_agent_session_context, plan_agent_session_selection,
};
use hq_domain::{
    AgentId, CommandDigest, CommandId, FactId, InstallationId, MailboxAddress, MailboxKind,
    OperationId, ProviderId, ProviderSessionId, RepositoryContext, ResourceLocator, Timestamp,
};
use hq_reducer::{
    AgentLifecycle, AgentProjection, AgentProjectionKey, AuthorityProjection,
    AuthorityProjectionKey, SessionIdentity,
};
use hq_resources::PathResourceAdapter;
use sha2::{Digest, Sha256};

/// Validated canonical evidence and repository context captured before provider I/O.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedAgentSessionSelection {
    /// Exact active named agent.
    pub agent_id: AgentId,
    /// Unique installation-local mailbox.
    pub mailbox: MailboxAddress,
    /// Exact compatible permanent name claim.
    pub claim_fact: FactId,
    /// Exact agent-mailbox creation fact.
    pub mailbox_root: FactId,
    /// Validated repository and launch-directory context.
    pub context: RepositoryContext,
}

#[derive(Clone, Copy)]
struct AgentEvidence {
    agent_id: AgentId,
    mailbox: MailboxAddress,
    claim_fact: FactId,
    mailbox_root: FactId,
}

enum CanonicalFactOutcome {
    Ready(FactId),
    Halt(AgentSessionSelectionOutcome),
}

/// Result of reconciling canonical facts after provider readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentSessionSelectionOutcome {
    /// Binding, context, and exact selection are durably projected.
    Complete,
    /// A canonical mutation is explicitly uncertain and exact replay is required.
    Uncertain,
    /// Current canonical authority or identity rejects the selection.
    Rejected,
}

/// Narrow canonical capability consumed by managed-session control.
pub trait AgentSessionCanonicalPort: Send + Sync {
    /// Validates active named-agent authority and resolves one absolute launch directory.
    fn prepare(
        &self,
        agent_id: AgentId,
        provider: &ProviderId,
        resume_session: Option<&ProviderSessionId>,
        directory: &ResourceLocator,
    ) -> Result<PreparedAgentSessionSelection, ApplicationError>;

    /// Reconciles immutable binding/context facts and selects the exact ready session.
    fn select_ready(
        &self,
        operation_id: OperationId,
        request_digest: CommandDigest,
        issued_at: Timestamp,
        prepared: &PreparedAgentSessionSelection,
        provider: &ProviderId,
        session: &ProviderSessionId,
    ) -> Result<AgentSessionSelectionOutcome, ApplicationError>;
}

/// Application/store-backed canonical named-agent session adapter.
pub struct ApplicationAgentSessionCanonicalPort<P> {
    ports: P,
    home: InstallationId,
    resources: PathResourceAdapter,
}

impl<P> ApplicationAgentSessionCanonicalPort<P> {
    /// Composes canonical application capabilities with local path observation.
    pub fn new(ports: P, home: InstallationId) -> Self {
        Self {
            ports,
            home,
            resources: PathResourceAdapter::system(),
        }
    }
}

impl<P> AgentSessionCanonicalPort for ApplicationAgentSessionCanonicalPort<P>
where
    P: QueryDomain + CommitFacts + Send + Sync,
{
    fn prepare(
        &self,
        agent_id: AgentId,
        provider: &ProviderId,
        resume_session: Option<&ProviderSessionId>,
        directory: &ResourceLocator,
    ) -> Result<PreparedAgentSessionSelection, ApplicationError> {
        let snapshot = self.ports.authoritative_snapshot()?;
        let evidence = agent_evidence(snapshot.domain(), self.home, agent_id)?;
        if let Some(session) = resume_session {
            let identity = SessionIdentity {
                provider: provider.clone(),
                session: session.clone(),
            };
            let Some(AgentProjection::Session(binding)) = snapshot
                .domain()
                .agent()
                .projection(AgentProjectionKey::Session(identity))
            else {
                return Err(identity_conflict());
            };
            if binding.conflicted || binding.mailbox != Some(evidence.mailbox) {
                return Err(identity_conflict());
            }
        }
        let path = PathBuf::from(directory.value());
        if !path.is_absolute() {
            return Err(invalid_request());
        }
        let context = self
            .resources
            .repository_context(self.home, path)
            .map_err(|_| invalid_request())?;
        Ok(PreparedAgentSessionSelection {
            agent_id: evidence.agent_id,
            mailbox: evidence.mailbox,
            claim_fact: evidence.claim_fact,
            mailbox_root: evidence.mailbox_root,
            context,
        })
    }

    fn select_ready(
        &self,
        operation_id: OperationId,
        request_digest: CommandDigest,
        issued_at: Timestamp,
        prepared: &PreparedAgentSessionSelection,
        provider: &ProviderId,
        session: &ProviderSessionId,
    ) -> Result<AgentSessionSelectionOutcome, ApplicationError> {
        let current = self.ports.authoritative_snapshot()?;
        let evidence = agent_evidence(current.domain(), self.home, prepared.agent_id)?;
        if evidence.mailbox != prepared.mailbox
            || evidence.claim_fact != prepared.claim_fact
            || evidence.mailbox_root != prepared.mailbox_root
        {
            return Ok(AgentSessionSelectionOutcome::Rejected);
        }

        let binding_fact = match self.ensure_binding(
            operation_id,
            request_digest,
            issued_at,
            prepared,
            provider,
            session,
        )? {
            CanonicalFactOutcome::Ready(fact) => fact,
            CanonicalFactOutcome::Halt(outcome) => return Ok(outcome),
        };
        let context_fact =
            match self.ensure_context(operation_id, request_digest, issued_at, prepared)? {
                CanonicalFactOutcome::Ready(fact) => fact,
                CanonicalFactOutcome::Halt(outcome) => return Ok(outcome),
            };

        let current = self.ports.authoritative_snapshot()?;
        if exact_selection(
            current.domain(),
            prepared.agent_id,
            provider,
            session,
            &prepared.context,
        ) {
            return Ok(AgentSessionSelectionOutcome::Complete);
        }
        let selection_frontier = selection_frontier(current.domain(), prepared.agent_id)?;
        let prepared = prepared.clone();
        let provider = provider.clone();
        let session = session.clone();
        self.commit(
            operation_id,
            request_digest,
            b"session-selection",
            move |_snapshot, authority, inputs| {
                plan_agent_session_selection(
                    authority,
                    inputs,
                    AgentSessionSelectionRequest {
                        agent_id: prepared.agent_id,
                        mailbox: prepared.mailbox,
                        claim_fact: prepared.claim_fact,
                        provider,
                        session,
                        binding_fact,
                        context_fact,
                        context: prepared.context,
                        selection_frontier,
                    },
                )
                .map_err(|_| identity_error())
            },
            issued_at,
        )
    }
}

impl<P> ApplicationAgentSessionCanonicalPort<P>
where
    P: QueryDomain + CommitFacts,
{
    fn ensure_binding(
        &self,
        operation_id: OperationId,
        request_digest: CommandDigest,
        issued_at: Timestamp,
        prepared: &PreparedAgentSessionSelection,
        provider: &ProviderId,
        session: &ProviderSessionId,
    ) -> Result<CanonicalFactOutcome, ApplicationError> {
        let current = self.ports.authoritative_snapshot()?;
        if let Some(fact) = exact_binding(current.domain(), prepared.mailbox, provider, session)? {
            return Ok(CanonicalFactOutcome::Ready(fact));
        }
        let prepared_for_plan = prepared.clone();
        let provider_for_plan = provider.clone();
        let session_for_plan = session.clone();
        let outcome = self.commit(
            operation_id,
            request_digest,
            b"session-binding",
            move |snapshot, authority, inputs| {
                let evidence = agent_evidence(
                    snapshot,
                    authority.installation_id,
                    prepared_for_plan.agent_id,
                )
                .map_err(|_| identity_error())?;
                plan_agent_session_binding(
                    authority,
                    inputs,
                    AgentSessionBindingRequest {
                        mailbox: evidence.mailbox,
                        mailbox_root: evidence.mailbox_root,
                        provider: provider_for_plan,
                        session: session_for_plan,
                    },
                )
                .map_err(|_| identity_error())
            },
            issued_at,
        )?;
        if outcome != AgentSessionSelectionOutcome::Complete {
            return Ok(CanonicalFactOutcome::Halt(outcome));
        }
        let snapshot = self.ports.authoritative_snapshot()?;
        exact_binding(snapshot.domain(), prepared.mailbox, provider, session)?
            .map(CanonicalFactOutcome::Ready)
            .ok_or_else(identity_conflict)
    }

    fn ensure_context(
        &self,
        operation_id: OperationId,
        request_digest: CommandDigest,
        issued_at: Timestamp,
        prepared: &PreparedAgentSessionSelection,
    ) -> Result<CanonicalFactOutcome, ApplicationError> {
        let current = self.ports.authoritative_snapshot()?;
        if let Some(fact) = exact_context(current.domain(), prepared.mailbox, &prepared.context) {
            return Ok(CanonicalFactOutcome::Ready(fact));
        }
        let prepared_for_plan = prepared.clone();
        let outcome = self.commit(
            operation_id,
            request_digest,
            b"session-context",
            move |snapshot, authority, inputs| {
                let evidence = agent_evidence(
                    snapshot,
                    authority.installation_id,
                    prepared_for_plan.agent_id,
                )
                .map_err(|_| identity_error())?;
                plan_agent_session_context(
                    authority,
                    inputs,
                    AgentSessionContextRequest {
                        mailbox: evidence.mailbox,
                        mailbox_root: evidence.mailbox_root,
                        context: prepared_for_plan.context,
                    },
                )
                .map_err(|_| identity_error())
            },
            issued_at,
        )?;
        if outcome != AgentSessionSelectionOutcome::Complete {
            return Ok(CanonicalFactOutcome::Halt(outcome));
        }
        let snapshot = self.ports.authoritative_snapshot()?;
        exact_context(snapshot.domain(), prepared.mailbox, &prepared.context)
            .map(CanonicalFactOutcome::Ready)
            .ok_or_else(identity_conflict)
    }

    fn commit(
        &self,
        operation_id: OperationId,
        request_digest: CommandDigest,
        purpose: &'static [u8],
        decide: impl FnOnce(
            &hq_application::DomainSnapshot,
            LocalInstallationAuthority,
            LocalFactInputs,
        ) -> Result<hq_application::FactPlan, hq_domain::DomainError>
        + Send
        + 'static,
        authored_at: Timestamp,
    ) -> Result<AgentSessionSelectionOutcome, ApplicationError> {
        let command_id = CommandId::from_bytes(derived_identity(
            b"hq-managed-session-command\0",
            operation_id,
            request_digest,
            purpose,
        ));
        let digest = CommandDigest::from_bytes(derived_identity(
            b"hq-managed-session-mutation\0",
            operation_id,
            request_digest,
            purpose,
        ));
        let randomness = derived_identity(
            b"hq-managed-session-fact\0",
            operation_id,
            request_digest,
            purpose,
        );
        let home = self.home;
        let attempt =
            self.ports
                .commit_facts(FactMutation::new(command_id, digest, move |snapshot| {
                    let authority = local_authority(snapshot, home).map_err(|_| identity_error());
                    match authority.and_then(|authority| {
                        decide(
                            snapshot,
                            authority,
                            LocalFactInputs {
                                authored_at,
                                auxiliary_randomness: randomness,
                            },
                        )
                    }) {
                        Ok(plan) => MutationDecision::commit(plan),
                        Err(error) => MutationDecision::reject(error),
                    }
                }))?;
        Ok(match attempt {
            MutationAttempt::Uncertain { .. } => AgentSessionSelectionOutcome::Uncertain,
            MutationAttempt::Completed(receipt) => match receipt.outcome() {
                MutationOutcome::Committed => AgentSessionSelectionOutcome::Complete,
                MutationOutcome::Rejected(_) => AgentSessionSelectionOutcome::Rejected,
            },
        })
    }
}

fn agent_evidence(
    snapshot: &hq_application::DomainSnapshot,
    home: InstallationId,
    agent_id: AgentId,
) -> Result<AgentEvidence, ApplicationError> {
    let Some(AgentProjection::Agent(agent)) = snapshot
        .agent()
        .projection(AgentProjectionKey::Agent(agent_id))
    else {
        return Err(identity_conflict());
    };
    let (Some(claim_fact), Some(mailbox)) = (
        only(agent.claims.iter().copied()),
        only(agent.mailboxes.iter().copied()),
    ) else {
        return Err(identity_conflict());
    };
    if agent.lifecycle != AgentLifecycle::Active || mailbox.installation_id() != home {
        return Err(identity_conflict());
    }
    let Some(AuthorityProjection::Mailbox(mailbox_view)) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Mailbox(mailbox))
    else {
        return Err(identity_conflict());
    };
    if mailbox_view.kind != MailboxKind::Agent {
        return Err(identity_conflict());
    }
    Ok(AgentEvidence {
        agent_id,
        mailbox,
        claim_fact,
        mailbox_root: mailbox_view.create_fact,
    })
}

fn local_authority(
    snapshot: &hq_application::DomainSnapshot,
    home: InstallationId,
) -> Result<LocalInstallationAuthority, ApplicationError> {
    let Some(AuthorityProjection::Installation(installation)) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(home))
    else {
        return Err(identity_conflict());
    };
    Ok(LocalInstallationAuthority {
        installation_id: home,
        signing_key: installation.signing_key,
        root_fact: installation.root_fact,
    })
}

fn exact_binding(
    snapshot: &hq_application::DomainSnapshot,
    mailbox: MailboxAddress,
    provider: &ProviderId,
    session: &ProviderSessionId,
) -> Result<Option<FactId>, ApplicationError> {
    let identity = SessionIdentity {
        provider: provider.clone(),
        session: session.clone(),
    };
    let Some(AgentProjection::Session(binding)) = snapshot
        .agent()
        .projection(AgentProjectionKey::Session(identity))
    else {
        return Ok(None);
    };
    if binding.conflicted || binding.mailbox != Some(mailbox) {
        return Err(identity_conflict());
    }
    Ok(binding
        .bindings
        .iter()
        .find_map(|(fact, candidate)| (*candidate == mailbox).then_some(*fact)))
}

fn exact_context(
    snapshot: &hq_application::DomainSnapshot,
    mailbox: MailboxAddress,
    context: &RepositoryContext,
) -> Option<FactId> {
    let AgentProjection::Context(history) = snapshot
        .agent()
        .projection(AgentProjectionKey::Context(mailbox))?
    else {
        return None;
    };
    history
        .history
        .iter()
        .find_map(|(fact, candidate)| (candidate == context).then_some(*fact))
}

fn selection_frontier(
    snapshot: &hq_application::DomainSnapshot,
    agent_id: AgentId,
) -> Result<BTreeSet<FactId>, ApplicationError> {
    match snapshot
        .agent()
        .projection(AgentProjectionKey::Selection(agent_id))
    {
        None => Ok(BTreeSet::new()),
        Some(AgentProjection::Selection(selection)) => Ok(selection.frontier.clone()),
        Some(_) => Err(identity_conflict()),
    }
}

fn exact_selection(
    snapshot: &hq_application::DomainSnapshot,
    agent_id: AgentId,
    provider: &ProviderId,
    session: &ProviderSessionId,
    context: &RepositoryContext,
) -> bool {
    matches!(
        snapshot
            .agent()
            .projection(AgentProjectionKey::Selection(agent_id)),
        Some(AgentProjection::Selection(selection))
            if !selection.conflicted
                && selection.active.as_ref().is_some_and(|active| {
                    active.session.provider == *provider
                        && active.session.session == *session
                        && active.context == *context
                })
    )
}

fn only<T>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn derived_identity(
    domain: &[u8],
    operation_id: OperationId,
    request_digest: CommandDigest,
    purpose: &[u8],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(operation_id.as_bytes());
    digest.update(request_digest.as_bytes());
    digest.update(purpose);
    digest.finalize().into()
}

fn identity_error() -> hq_domain::DomainError {
    hq_domain::DomainError::new(
        hq_domain::ErrorCategory::Conflict,
        hq_domain::ErrorCode::new("managed_session_identity_conflict")
            .unwrap_or_else(|_| unreachable!()),
    )
}

const fn identity_conflict() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::StateIdentityConflict)
}

const fn invalid_request() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::InvalidRequest)
}
