//! Canonical application adapter for provider-neutral normalized harness values.

use std::{collections::BTreeSet, fmt, sync::Arc};

use hq_application::{
    ApplicationError, ApplicationErrorCode, CommitFacts, FactMutation, HarnessActivityFactRequest,
    HarnessAuthoringAuthority, HarnessOutputFactRequest, LocalFactInputs, MutationAttempt,
    MutationDecision, MutationOutcome, ProjectHarnessAuthoringAuthority, plan_harness_activity,
    plan_harness_output, plan_project_harness_activity, plan_project_harness_output,
};
use hq_domain::{
    ActivityKind, ActivityStatus, AgentId, AssignmentBinding, AuthorityReference, AuthorityRole,
    CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode, InstallationId,
    MailboxAddress, MailboxId, MessageId, OperationCorrelation, PresentationKind, ProviderId,
    ProviderSessionId, Timestamp,
};
use hq_harness::{
    HarnessActivity, HarnessClock, HarnessDeliveryRecord, HarnessError, HarnessErrorClass,
    HarnessOutput, HarnessOutputKind, HarnessPersistencePort,
};
use hq_reducer::{
    ActivityKey, AgentLifecycle, AgentProjection, AgentProjectionKey, AuthorityProjection,
    AuthorityProjectionKey, ConversationAggregateKey, ConversationProjection,
    ConversationProjectionKey, ProjectProjection, ProjectProjectionKey, SessionIdentity,
};
use sha2::{Digest, Sha256};

/// Node-owned canonical persistence capability for normalized provider values.
pub struct CanonicalHarnessPersistence<P> {
    ports: P,
    home: InstallationId,
    human_mailbox: MailboxId,
    clock: Arc<dyn HarnessClock>,
}

impl<P> CanonicalHarnessPersistence<P> {
    /// Binds canonical application mutation, local authority, and explicit time capabilities.
    pub const fn new(
        ports: P,
        home: InstallationId,
        human_mailbox: MailboxId,
        clock: Arc<dyn HarnessClock>,
    ) -> Self {
        Self {
            ports,
            home,
            human_mailbox,
            clock,
        }
    }
}

impl<P: CommitFacts + Send + Sync> HarnessPersistencePort for CanonicalHarnessPersistence<P> {
    fn persist_output(
        &self,
        agent_id: AgentId,
        provider_id: &ProviderId,
        session_id: &ProviderSessionId,
        delivery: Option<&HarnessDeliveryRecord>,
        output: &HarnessOutput,
    ) -> Result<(), HarnessError> {
        validate_delivery_runtime(
            agent_id,
            provider_id,
            session_id,
            delivery,
            output.operation_id,
        )?;
        let identity = output_identity(output.output_id);
        let digest = output_digest(agent_id, provider_id, session_id, delivery, output);
        let authored_at = timestamp(self.clock.now_millis());
        let randomness = fact_randomness(identity, digest);
        let home = self.home;
        let human_mailbox = self.human_mailbox;
        let provider = provider_id.clone();
        let session = session_id.clone();
        let delivery = delivery.cloned();
        let output = output.clone();
        self.commit(identity, digest, move |snapshot| {
            let request = HarnessOutputFactRequest {
                output_id: output.output_id,
                correlation: OperationCorrelation::new(
                    provider.clone(),
                    session.clone(),
                    output.operation_id,
                ),
                presentation: match output.kind {
                    HarnessOutputKind::Update => PresentationKind::Status,
                    HarnessOutputKind::FinalAnswer => PresentationKind::FinalAnswer,
                },
                body: output.body,
            };
            if let Some(delivery) = delivery.as_ref() {
                let authority = project_authoring_authority(snapshot, home, delivery, None)?;
                return plan_project_harness_output(
                    &authority,
                    LocalFactInputs {
                        authored_at,
                        auxiliary_randomness: randomness,
                    },
                    request,
                );
            }
            let authority = authoring_authority(
                snapshot,
                home,
                human_mailbox,
                agent_id,
                &provider,
                &session,
                None,
            )?;
            plan_harness_output(
                &authority,
                LocalFactInputs {
                    authored_at,
                    auxiliary_randomness: randomness,
                },
                request,
            )
        })
    }

    fn persist_activity(
        &self,
        agent_id: AgentId,
        provider_id: &ProviderId,
        session_id: &ProviderSessionId,
        delivery: Option<&HarnessDeliveryRecord>,
        activity: &HarnessActivity,
    ) -> Result<(), HarnessError> {
        validate_delivery_runtime(
            agent_id,
            provider_id,
            session_id,
            delivery,
            activity.operation_id,
        )?;
        let identity = activity_identity(agent_id, provider_id, session_id, activity);
        let digest = activity_digest(agent_id, provider_id, session_id, delivery, activity);
        let occurred_at = timestamp(self.clock.now_millis());
        let randomness = fact_randomness(identity, digest);
        let home = self.home;
        let human_mailbox = self.human_mailbox;
        let provider = provider_id.clone();
        let session = session_id.clone();
        let delivery = delivery.cloned();
        let activity = activity.clone();
        self.commit(identity, digest, move |snapshot| {
            let correlation =
                OperationCorrelation::new(provider.clone(), session.clone(), activity.operation_id);
            let key = ActivityKey {
                source: source_mailbox(snapshot, home, agent_id)?,
                correlation: correlation.clone(),
                item: activity.item.clone(),
                kind: activity.kind,
                logical_key: activity.logical_key.clone(),
                runtime: activity.runtime.clone(),
            };
            let request = HarnessActivityFactRequest {
                correlation,
                item: activity.item,
                kind: activity.kind,
                logical_key: activity.logical_key,
                runtime: activity.runtime,
                sequence: activity.sequence,
                occurred_at,
                status: activity.status,
                content: activity.content,
                truncated: activity.truncated,
            };
            let inputs = LocalFactInputs {
                authored_at: occurred_at,
                auxiliary_randomness: randomness,
            };
            if let Some(delivery) = delivery.as_ref() {
                let authority = project_authoring_authority(snapshot, home, delivery, Some(&key))?;
                return plan_project_harness_activity(&authority, inputs, request);
            }
            let authority = authoring_authority(
                snapshot,
                home,
                human_mailbox,
                agent_id,
                &provider,
                &session,
                Some(&key),
            )?;
            plan_harness_activity(&authority, inputs, request)
        })
    }
}

impl<P: CommitFacts> CanonicalHarnessPersistence<P> {
    fn commit(
        &self,
        command_id: CommandId,
        request_digest: CommandDigest,
        decide: impl FnOnce(
            &hq_application::DomainSnapshot,
        ) -> Result<hq_application::FactPlan, ApplicationError>
        + Send
        + 'static,
    ) -> Result<(), HarnessError> {
        let attempt = self
            .ports
            .commit_facts(FactMutation::new(
                command_id,
                request_digest,
                move |snapshot| match decide(snapshot) {
                    Ok(plan) => MutationDecision::commit(plan),
                    Err(_) => MutationDecision::reject(persistence_rejection()),
                },
            ))
            .map_err(|error| {
                if error.code() == ApplicationErrorCode::StateIdentityConflict {
                    collision()
                } else {
                    unavailable()
                }
            })?;
        match attempt {
            MutationAttempt::Completed(receipt)
                if matches!(receipt.outcome(), MutationOutcome::Committed) =>
            {
                Ok(())
            }
            MutationAttempt::Completed(_) => Err(collision()),
            MutationAttempt::Uncertain { .. } => Err(unavailable()),
        }
    }
}

fn authoring_authority(
    snapshot: &hq_application::DomainSnapshot,
    home: InstallationId,
    human_mailbox: MailboxId,
    agent_id: AgentId,
    provider: &ProviderId,
    session: &ProviderSessionId,
    activity_key: Option<&ActivityKey>,
) -> Result<HarnessAuthoringAuthority, ApplicationError> {
    let source = source_mailbox(snapshot, home, agent_id)?;
    let Some(AuthorityProjection::Installation(installation)) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(home))
    else {
        return Err(identity_error());
    };
    let recipient = MailboxAddress::new(home, human_mailbox);
    let Some(AuthorityProjection::Mailbox(human)) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Mailbox(recipient))
    else {
        return Err(identity_error());
    };
    if human.kind != hq_domain::MailboxKind::Human {
        return Err(identity_error());
    }
    let Some(AuthorityProjection::Mailbox(agent_mailbox)) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Mailbox(source))
    else {
        return Err(identity_error());
    };
    if agent_mailbox.kind != hq_domain::MailboxKind::Agent {
        return Err(identity_error());
    }

    let identity = SessionIdentity {
        provider: provider.clone(),
        session: session.clone(),
    };
    let mut support = BTreeSet::from([
        installation.root_fact,
        human.create_fact,
        agent_mailbox.create_fact,
    ]);
    let mut bound = false;
    if let Some(AgentProjection::Session(binding)) = snapshot
        .agent()
        .projection(AgentProjectionKey::Session(identity.clone()))
    {
        if binding.conflicted || binding.mailbox != Some(source) {
            return Err(identity_error());
        }
        support.extend(binding.bindings.keys().copied());
        bound = true;
    }
    for projection in snapshot.project().projections().values() {
        let ProjectProjection::Project(project) = projection else {
            continue;
        };
        let Some(assignment) = project.assignment.as_ref() else {
            continue;
        };
        if assignment.runnable
            && !assignment.cardinality_conflicted
            && assignment.binding.as_ref().is_some_and(|binding| {
                binding.agent_id == agent_id
                    && binding.provider == *provider
                    && binding.session == *session
            })
        {
            support.extend(assignment.support.iter().copied());
            bound = true;
        }
    }
    if !bound {
        return Err(identity_error());
    }
    if let Some(key) = activity_key {
        if let Some(ConversationProjection::Activity(previous)) = snapshot
            .conversation()
            .projection(ConversationProjectionKey::Activity(key.clone()))
        {
            support.insert(previous.fact_id);
        }
        if let Some(frontier) = snapshot
            .conversation()
            .frontiers()
            .get(&ConversationAggregateKey::Activity(key.clone()))
        {
            support.extend(frontier.iter().copied());
        }
    }
    Ok(HarnessAuthoringAuthority {
        author: home,
        source,
        recipient,
        authority: AuthorityReference::new(
            AuthorityRole::LocalInstallation,
            installation.root_fact,
        ),
        support,
    })
}

fn project_authoring_authority(
    snapshot: &hq_application::DomainSnapshot,
    home: InstallationId,
    delivery: &HarnessDeliveryRecord,
    activity_key: Option<&ActivityKey>,
) -> Result<ProjectHarnessAuthoringAuthority, ApplicationError> {
    let project_delivery = delivery.project.as_ref().ok_or_else(identity_error)?;
    let agent_id = delivery.agent_id;
    let provider = &delivery.provider_id;
    let session = &delivery.session_id;
    let Some(ProjectProjection::Project(project)) = snapshot
        .project()
        .projection(ProjectProjectionKey::Project(project_delivery.project_id))
    else {
        return Err(identity_error());
    };
    if project.home != home || !project.fork_participants.is_empty() {
        return Err(identity_error());
    }
    let Some(ProjectProjection::Dispatch(dispatch)) = snapshot
        .project()
        .projection(ProjectProjectionKey::Dispatch(project_delivery.dispatch_id))
    else {
        return Err(identity_error());
    };
    let binding = AssignmentBinding {
        assignment_id: project_delivery.assignment_id,
        agent_id,
        provider: provider.clone(),
        session: session.clone(),
    };
    if dispatch.conflicted
        || dispatch.message_id != delivery.submission.submission_id
        || dispatch.sequence != project_delivery.sequence.get()
        || dispatch.binding != binding
        || dispatch.thread_id != project_delivery.thread_id
        || !matches!(
            snapshot
                .project()
                .projection(ProjectProjectionKey::Input(dispatch.message_id)),
            Some(ProjectProjection::Input(input))
                if input.project_id == project_delivery.project_id
                    && input.sequence == project_delivery.sequence.get()
        )
    {
        return Err(identity_error());
    }
    let source = source_mailbox(snapshot, home, agent_id)?;
    let Some(AuthorityProjection::Installation(installation)) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(home))
    else {
        return Err(identity_error());
    };
    let account_membership = active_account_authority(snapshot, project.account_id, home)?;
    let Some(AuthorityProjection::Mailbox(agent_mailbox)) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Mailbox(source))
    else {
        return Err(identity_error());
    };
    if agent_mailbox.kind != hq_domain::MailboxKind::Agent {
        return Err(identity_error());
    }
    let mut support = BTreeSet::from([
        installation.root_fact,
        account_membership,
        agent_mailbox.create_fact,
        project.root,
        dispatch.fact_id,
    ]);
    if let Some(key) = activity_key {
        if let Some(ConversationProjection::Activity(previous)) = snapshot
            .conversation()
            .projection(ConversationProjectionKey::Activity(key.clone()))
        {
            support.insert(previous.fact_id);
        }
        if let Some(frontier) = snapshot
            .conversation()
            .frontiers()
            .get(&ConversationAggregateKey::Activity(key.clone()))
        {
            support.extend(frontier.iter().copied());
        }
    }
    Ok(ProjectHarnessAuthoringAuthority {
        author: home,
        account_id: project.account_id,
        project_id: project_delivery.project_id,
        project_head: project.head,
        installation_root: installation.root_fact,
        account_membership,
        dispatch_fact: dispatch.fact_id,
        dispatch_id: project_delivery.dispatch_id,
        source,
        recipient: project.mailbox,
        binding,
        thread_id: project_delivery.thread_id,
        support,
    })
}

fn active_account_authority(
    snapshot: &hq_application::DomainSnapshot,
    account_id: hq_domain::AccountId,
    home: InstallationId,
) -> Result<hq_domain::FactId, ApplicationError> {
    if let Some(AuthorityProjection::Account {
        root_fact, creator, ..
    }) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Account(account_id))
        && creator.installation_id() == home
    {
        return Ok(*root_fact);
    }
    match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Membership {
            account: account_id,
            device: home,
        }) {
        Some(AuthorityProjection::Membership(membership))
            if membership.state() == hq_reducer::MembershipState::Active =>
        {
            membership
                .active_acceptances
                .iter()
                .next()
                .copied()
                .ok_or_else(identity_error)
        }
        _ => Err(identity_error()),
    }
}

fn validate_delivery_runtime(
    agent_id: AgentId,
    provider: &ProviderId,
    session: &ProviderSessionId,
    delivery: Option<&HarnessDeliveryRecord>,
    operation_id: hq_domain::OperationId,
) -> Result<(), HarnessError> {
    let Some(delivery) = delivery else {
        return Ok(());
    };
    if delivery.agent_id != agent_id
        || delivery.provider_id != *provider
        || delivery.session_id != *session
        || delivery.submission.operation_id != operation_id
        || delivery.project.is_none()
    {
        return Err(collision());
    }
    Ok(())
}

fn source_mailbox(
    snapshot: &hq_application::DomainSnapshot,
    home: InstallationId,
    agent_id: AgentId,
) -> Result<MailboxAddress, ApplicationError> {
    let Some(AgentProjection::Agent(agent)) = snapshot
        .agent()
        .projection(AgentProjectionKey::Agent(agent_id))
    else {
        return Err(identity_error());
    };
    if agent.lifecycle != AgentLifecycle::Active || agent.mailboxes.len() != 1 {
        return Err(identity_error());
    }
    agent
        .mailboxes
        .iter()
        .next()
        .copied()
        .filter(|mailbox| mailbox.installation_id() == home)
        .ok_or_else(identity_error)
}

fn output_identity(output_id: MessageId) -> CommandId {
    let mut digest = Sha256::new();
    digest.update(b"hq-harness-output-command-v1\0");
    digest.update(output_id.as_bytes());
    CommandId::from_bytes(digest.finalize().into())
}

fn output_digest(
    agent_id: AgentId,
    provider: &ProviderId,
    session: &ProviderSessionId,
    delivery: Option<&HarnessDeliveryRecord>,
    output: &HarnessOutput,
) -> CommandDigest {
    let mut digest = Sha256::new();
    digest.update(b"hq-harness-output-request-v1\0");
    digest.update(agent_id.as_bytes());
    update_text(&mut digest, provider.as_str());
    update_text(&mut digest, session.as_str());
    digest.update(output.output_id.as_bytes());
    digest.update(output.operation_id.as_bytes());
    digest.update([match output.kind {
        HarnessOutputKind::Update => 1,
        HarnessOutputKind::FinalAnswer => 2,
    }]);
    update_status(&mut digest, &output.status);
    update_text(&mut digest, output.body.as_str());
    update_delivery_attribution(&mut digest, delivery);
    CommandDigest::from_bytes(digest.finalize().into())
}

fn activity_identity(
    agent_id: AgentId,
    provider: &ProviderId,
    session: &ProviderSessionId,
    activity: &HarnessActivity,
) -> CommandId {
    let mut digest = Sha256::new();
    digest.update(b"hq-harness-activity-command-v1\0");
    digest.update(agent_id.as_bytes());
    update_text(&mut digest, provider.as_str());
    update_text(&mut digest, session.as_str());
    digest.update(activity.operation_id.as_bytes());
    update_optional_text(
        &mut digest,
        activity.item.as_ref().map(hq_domain::BoundedText::as_str),
    );
    digest.update([activity_kind(activity.kind)]);
    update_text(&mut digest, activity.logical_key.as_str());
    update_text(&mut digest, activity.runtime.as_str());
    digest.update(activity.sequence.get().to_be_bytes());
    CommandId::from_bytes(digest.finalize().into())
}

fn activity_digest(
    agent_id: AgentId,
    provider: &ProviderId,
    session: &ProviderSessionId,
    delivery: Option<&HarnessDeliveryRecord>,
    activity: &HarnessActivity,
) -> CommandDigest {
    let identity = activity_identity(agent_id, provider, session, activity);
    let mut digest = Sha256::new();
    digest.update(b"hq-harness-activity-request-v1\0");
    digest.update(identity.as_bytes());
    update_status(&mut digest, &activity.status);
    update_text(&mut digest, activity.content.as_str());
    digest.update([u8::from(activity.truncated)]);
    update_delivery_attribution(&mut digest, delivery);
    CommandDigest::from_bytes(digest.finalize().into())
}

fn update_delivery_attribution(digest: &mut Sha256, delivery: Option<&HarnessDeliveryRecord>) {
    let Some(delivery) = delivery else {
        return;
    };
    let Some(project) = delivery.project.as_ref() else {
        digest.update([0]);
        return;
    };
    digest.update([1]);
    digest.update(delivery.submission.submission_id.as_bytes());
    digest.update(project.project_id.as_bytes());
    digest.update(project.dispatch_id.as_bytes());
    digest.update(project.assignment_id.as_bytes());
    digest.update(project.thread_id.as_bytes());
    digest.update(project.sequence.get().to_be_bytes());
}

fn fact_randomness(command: CommandId, digest: CommandDigest) -> [u8; 32] {
    let mut value = Sha256::new();
    value.update(b"hq-harness-canonical-fact-v1\0");
    value.update(command.as_bytes());
    value.update(digest.as_bytes());
    value.finalize().into()
}

fn timestamp(millis: u64) -> Timestamp {
    Timestamp::from_unix_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

fn update_text(digest: &mut Sha256, value: &str) {
    digest.update(value.len().to_be_bytes());
    digest.update(value.as_bytes());
}

fn update_optional_text(digest: &mut Sha256, value: Option<&str>) {
    digest.update([u8::from(value.is_some())]);
    if let Some(value) = value {
        update_text(digest, value);
    }
}

fn update_status(digest: &mut Sha256, status: &ActivityStatus) {
    match status {
        ActivityStatus::Snapshot => digest.update([1]),
        ActivityStatus::Running => digest.update([2]),
        ActivityStatus::Succeeded => digest.update([3]),
        ActivityStatus::Failed(code) => {
            digest.update([4]);
            update_text(digest, code.as_str());
        }
        ActivityStatus::Interrupted => digest.update([5]),
    }
}

const fn activity_kind(kind: ActivityKind) -> u8 {
    match kind {
        ActivityKind::Status => 1,
        ActivityKind::Progress => 2,
        ActivityKind::Plan => 3,
        ActivityKind::Diff => 4,
        ActivityKind::CompletedItem => 5,
    }
}

fn persistence_rejection() -> DomainError {
    DomainError::new(
        ErrorCategory::Conflict,
        ErrorCode::new("harness_persistence_stale_binding")
            .unwrap_or_else(|_| unreachable!("static error code is valid")),
    )
}

const fn identity_error() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::StateIdentityConflict)
}

const fn unavailable() -> HarnessError {
    HarnessError::new(HarnessErrorClass::Unavailable)
}

const fn collision() -> HarnessError {
    HarnessError::new(HarnessErrorClass::PersistenceCollision)
}

impl<P> fmt::Debug for CanonicalHarnessPersistence<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalHarnessPersistence")
            .field("home", &self.home)
            .field("human_mailbox", &self.human_mailbox)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use std::{
        collections::{BTreeMap, BTreeSet},
        num::NonZeroU64,
        sync::Mutex,
    };

    use hq_application::{
        AgentProjectionSnapshot, AuthorityProjectionSnapshot, ConversationProjectionSnapshot,
        DomainSnapshot, MutationReceipt, ProjectProjectionSnapshot, ProjectionSnapshot,
    };
    use hq_domain::{
        AccountId, AssignmentBinding, AssignmentId, AssignmentIntent, BoundedText, ContentText,
        DispatchId, EncryptionPublicKey, FactId, MailboxKind, OperationId, ProjectId,
        ResourceLocator, ResourceScheme, Revision, ShortText, SigningPublicKey, ThreadId,
    };
    use hq_harness::{HarnessDeliveryState, HarnessProjectDelivery, HarnessSubmission};
    use hq_reducer::{
        AgentView, AuthorityProjection, InstallationView, MailboxView, ProjectAssignmentPhase,
        ProjectAssignmentView, ProjectDispatchView, ProjectInputView, ProjectLifecycle,
        ProjectProjectionKey, ProjectView, SessionBindingView,
    };

    use super::*;

    #[derive(Clone)]
    struct Ports(Arc<PortsState>);

    struct PortsState {
        snapshot: DomainSnapshot,
        retained: Mutex<BTreeMap<CommandId, (CommandDigest, hq_domain::SemanticPayload)>>,
    }

    impl CommitFacts for Ports {
        fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
            let (command, digest, decide) = request.into_parts();
            let mut retained = self.0.retained.lock().expect("retained lock");
            if let Some((stored, _)) = retained.get(&command) {
                if stored != &digest {
                    return Err(ApplicationError::new(
                        ApplicationErrorCode::StateIdentityConflict,
                    ));
                }
                return Ok(committed(command, digest));
            }
            match decide(&self.0.snapshot) {
                MutationDecision::Commit(plan) => {
                    retained.insert(command, (digest, plan.payload().clone()));
                    Ok(committed(command, digest))
                }
                MutationDecision::Reject(error) => {
                    Ok(MutationAttempt::Completed(MutationReceipt::new(
                        command,
                        digest,
                        Revision::new(1),
                        MutationOutcome::Rejected(error),
                    )))
                }
            }
        }
    }

    struct Clock;

    impl HarnessClock for Clock {
        fn now_millis(&self) -> u64 {
            123
        }
    }

    struct Fixture {
        ports: Ports,
        persistence: CanonicalHarnessPersistence<Ports>,
        agent: AgentId,
        provider: ProviderId,
        session: ProviderSessionId,
        source: MailboxAddress,
    }

    #[allow(clippy::too_many_lines)]
    fn fixture(bound: bool) -> Fixture {
        let home = InstallationId::from_bytes([1; 32]);
        let human_id = MailboxId::from_bytes([2; 32]);
        let agent = AgentId::from_bytes([3; 32]);
        let source = MailboxAddress::new(home, MailboxId::from_bytes([4; 32]));
        let recipient = MailboxAddress::new(home, human_id);
        let provider = ProviderId::new("provider").expect("provider");
        let session = ProviderSessionId::new("session").expect("session");
        let identity = SessionIdentity {
            provider: provider.clone(),
            session: session.clone(),
        };
        let authority = AuthorityProjectionSnapshot::new(
            BTreeMap::new(),
            BTreeMap::from([
                (
                    AuthorityProjectionKey::Installation(home),
                    AuthorityProjection::Installation(InstallationView {
                        root_fact: FactId::from_bytes([5; 32]),
                        signing_key: SigningPublicKey::from_bytes([6; 32]),
                        encryption_key: EncryptionPublicKey::from_bytes([7; 32]),
                        label: None,
                    }),
                ),
                (
                    AuthorityProjectionKey::Account(AccountId::from_bytes([20; 32])),
                    AuthorityProjection::Account {
                        root_fact: FactId::from_bytes([30; 32]),
                        creator: hq_domain::InstallationAddress::new(
                            home,
                            SigningPublicKey::from_bytes([6; 32]),
                        ),
                        label: None,
                    },
                ),
                (
                    AuthorityProjectionKey::Mailbox(recipient),
                    AuthorityProjection::Mailbox(MailboxView {
                        create_fact: FactId::from_bytes([8; 32]),
                        kind: MailboxKind::Human,
                        label: None,
                    }),
                ),
                (
                    AuthorityProjectionKey::Mailbox(source),
                    AuthorityProjection::Mailbox(MailboxView {
                        create_fact: FactId::from_bytes([9; 32]),
                        kind: MailboxKind::Agent,
                        label: None,
                    }),
                ),
                (
                    AuthorityProjectionKey::Mailbox(MailboxAddress::new(
                        home,
                        MailboxId::from_bytes([19; 32]),
                    )),
                    AuthorityProjection::Mailbox(MailboxView {
                        create_fact: FactId::from_bytes([31; 32]),
                        kind: MailboxKind::Human,
                        label: None,
                    }),
                ),
            ]),
            BTreeMap::new(),
        );
        let mut agent_projections = BTreeMap::from([(
            AgentProjectionKey::Agent(agent),
            AgentProjection::Agent(Box::new(AgentView {
                claims: BTreeSet::from([FactId::from_bytes([10; 32])]),
                names: BTreeSet::from([ShortText::new("worker").expect("name")]),
                mailboxes: BTreeSet::from([source]),
                retirements: BTreeSet::new(),
                lifecycle: AgentLifecycle::Active,
                runnable: bound,
                selected_session: bound.then_some(identity.clone()),
                name_reserved: true,
            })),
        )]);
        if bound {
            agent_projections.insert(
                AgentProjectionKey::Session(identity),
                AgentProjection::Session(Box::new(SessionBindingView {
                    bindings: BTreeMap::from([(FactId::from_bytes([11; 32]), source)]),
                    conflicted: false,
                    mailbox: Some(source),
                })),
            );
        }
        let snapshot = DomainSnapshot::new(
            authority,
            empty_conversation(),
            AgentProjectionSnapshot::new(BTreeMap::new(), agent_projections, BTreeMap::new()),
            empty_project(),
        );
        let ports = Ports(Arc::new(PortsState {
            snapshot,
            retained: Mutex::new(BTreeMap::new()),
        }));
        let persistence =
            CanonicalHarnessPersistence::new(ports.clone(), home, human_id, Arc::new(Clock));
        Fixture {
            ports,
            persistence,
            agent,
            provider,
            session,
            source,
        }
    }

    fn empty_conversation() -> ConversationProjectionSnapshot {
        ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new())
    }

    fn empty_project() -> ProjectProjectionSnapshot {
        ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new())
    }

    fn committed(command: CommandId, digest: CommandDigest) -> MutationAttempt {
        MutationAttempt::Completed(MutationReceipt::new(
            command,
            digest,
            Revision::new(1),
            MutationOutcome::Committed,
        ))
    }

    fn output(body: &str) -> HarnessOutput {
        HarnessOutput {
            output_id: MessageId::from_bytes([12; 32]),
            operation_id: OperationId::from_bytes([13; 32]),
            kind: HarnessOutputKind::FinalAnswer,
            status: ActivityStatus::Succeeded,
            body: ContentText::new(body).expect("body"),
        }
    }

    #[test]
    fn output_is_idempotent_correlated_and_changed_identity_collides() {
        let fixture = fixture(true);
        fixture
            .persistence
            .persist_output(
                fixture.agent,
                &fixture.provider,
                &fixture.session,
                None,
                &output("done"),
            )
            .expect("first output");
        fixture
            .persistence
            .persist_output(
                fixture.agent,
                &fixture.provider,
                &fixture.session,
                None,
                &output("done"),
            )
            .expect("duplicate output");
        let retained = fixture.ports.0.retained.lock().expect("retained lock");
        assert_eq!(retained.len(), 1);
        let payload = &retained.values().next().expect("payload").1;
        assert!(matches!(
            payload,
            hq_domain::SemanticPayload::AsynchronousMessageSent(message)
                if message.sender == fixture.source
                    && message.correlation.as_ref().is_some_and(|value| {
                        value.provider() == &fixture.provider
                            && value.session() == &fixture.session
                    })
                    && message.presentation == PresentationKind::FinalAnswer
        ));
        drop(retained);

        let error = fixture
            .persistence
            .persist_output(
                fixture.agent,
                &fixture.provider,
                &fixture.session,
                None,
                &output("changed"),
            )
            .expect_err("changed output collides");
        assert_eq!(error.class, HarnessErrorClass::PersistenceCollision);
    }

    #[test]
    fn activity_preserves_normalized_fields_and_stale_binding_is_redacted() {
        let active = fixture(true);
        let activity = HarnessActivity {
            operation_id: OperationId::from_bytes([14; 32]),
            item: Some(ShortText::new("item").expect("item")),
            kind: ActivityKind::Progress,
            logical_key: ShortText::new("plan").expect("key"),
            runtime: ShortText::new("runtime").expect("runtime"),
            sequence: NonZeroU64::new(2).expect("sequence"),
            status: ActivityStatus::Running,
            content: ContentText::new("secret diagnostic text").expect("content"),
            truncated: true,
        };
        active
            .persistence
            .persist_activity(
                active.agent,
                &active.provider,
                &active.session,
                None,
                &activity,
            )
            .expect("activity");
        let retained = active.ports.0.retained.lock().expect("retained lock");
        assert!(retained.values().any(|(_, payload)| matches!(
            payload,
            hq_domain::SemanticPayload::HarnessActivityRecorded {
                sequence,
                truncated: true,
                ..
            } if *sequence == activity.sequence
        )));
        drop(retained);

        let stale = fixture(false);
        let error = stale
            .persistence
            .persist_activity(
                stale.agent,
                &stale.provider,
                &stale.session,
                None,
                &activity,
            )
            .expect_err("stale binding rejects");
        assert_eq!(error.class, HarnessErrorClass::PersistenceCollision);
        assert!(!format!("{error:?}").contains("secret diagnostic text"));
        assert!(!format!("{:?}", stale.persistence).contains("secret diagnostic text"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn attributed_project_events_preserve_exact_dispatch_provenance() {
        let stale = fixture(false);
        let home = stale.source.installation_id();
        let assignment_id = AssignmentId::from_bytes([15; 32]);
        let project_id = ProjectId::from_bytes([16; 32]);
        let binding = AssignmentBinding {
            assignment_id,
            agent_id: stale.agent,
            provider: stale.provider.clone(),
            session: stale.session.clone(),
        };
        let project = ProjectView {
            root: FactId::from_bytes([17; 32]),
            head: FactId::from_bytes([18; 32]),
            fork_participants: BTreeSet::new(),
            home,
            account_id: AccountId::from_bytes([20; 32]),
            mailbox: MailboxAddress::new(home, MailboxId::from_bytes([19; 32])),
            predecessor: None,
            name: ShortText::new("project").expect("project name"),
            brief: None,
            resources: BTreeMap::new(),
            primary: None,
            lifecycle: ProjectLifecycle::Open,
            archived: false,
            active_claims: BTreeSet::new(),
            claim_conflicts: BTreeMap::new(),
            claimable: true,
            assignment: Some(ProjectAssignmentView {
                intent: AssignmentIntent {
                    assignment_id,
                    agent_id: stale.agent,
                    provider: stale.provider.clone(),
                },
                binding: Some(binding),
                phase: ProjectAssignmentPhase::Runnable {
                    thread_id: ThreadId::from_bytes([20; 32]),
                    launch_directory: ResourceLocator::new(
                        ResourceScheme::WorkingTree,
                        BoundedText::new("/work/project").expect("directory"),
                    ),
                },
                cardinality_conflicted: false,
                runnable: true,
                support: BTreeSet::from([FactId::from_bytes([18; 32])]),
            }),
            input_sequence: 1,
        };
        let submission_id = MessageId::from_bytes([21; 32]);
        let dispatch_id = DispatchId::from_bytes([22; 32]);
        let dispatch_fact = FactId::from_bytes([23; 32]);
        let thread_id = ThreadId::from_bytes([20; 32]);
        let snapshot = DomainSnapshot::new(
            stale.ports.0.snapshot.authority().clone(),
            stale.ports.0.snapshot.conversation().clone(),
            stale.ports.0.snapshot.agent().clone(),
            ProjectProjectionSnapshot::new(
                BTreeMap::new(),
                BTreeMap::from([
                    (
                        ProjectProjectionKey::Project(project_id),
                        ProjectProjection::Project(Box::new(project)),
                    ),
                    (
                        ProjectProjectionKey::Input(submission_id),
                        ProjectProjection::Input(Box::new(ProjectInputView {
                            project_id,
                            message_id: submission_id,
                            input_fact_id: FactId::from_bytes([24; 32]),
                            sequence: 1,
                            accepted_fact: FactId::from_bytes([25; 32]),
                        })),
                    ),
                    (
                        ProjectProjectionKey::Dispatch(dispatch_id),
                        ProjectProjection::Dispatch(Box::new(ProjectDispatchView {
                            dispatch_id,
                            message_id: submission_id,
                            sequence: 1,
                            binding: AssignmentBinding {
                                assignment_id,
                                agent_id: stale.agent,
                                provider: stale.provider.clone(),
                                session: stale.session.clone(),
                            },
                            thread_id,
                            fact_id: dispatch_fact,
                            conflicted: false,
                        })),
                    ),
                ]),
                BTreeMap::new(),
            ),
        );
        let ports = Ports(Arc::new(PortsState {
            snapshot,
            retained: Mutex::new(BTreeMap::new()),
        }));
        let persistence = CanonicalHarnessPersistence::new(
            ports.clone(),
            home,
            MailboxId::from_bytes([2; 32]),
            Arc::new(Clock),
        );
        let delivery = HarnessDeliveryRecord {
            agent_id: stale.agent,
            provider_id: stale.provider.clone(),
            session_id: stale.session.clone(),
            submission: HarnessSubmission {
                submission_id,
                digest: CommandDigest::from_bytes([26; 32]),
                operation_id: OperationId::from_bytes([13; 32]),
                body: ContentText::new("project input").expect("input"),
            },
            project: Some(HarnessProjectDelivery {
                project_id,
                dispatch_id,
                assignment_id,
                thread_id,
                sequence: NonZeroU64::MIN,
            }),
            queued_at_millis: 1,
            state: HarnessDeliveryState::Accepted,
        };
        let resolved = project_authoring_authority(&ports.0.snapshot, home, &delivery, None)
            .expect("project authority resolves");
        plan_project_harness_output(
            &resolved,
            LocalFactInputs {
                authored_at: Timestamp::from_unix_millis(123),
                auxiliary_randomness: [1; 32],
            },
            HarnessOutputFactRequest {
                output_id: output("project output").output_id,
                correlation: OperationCorrelation::new(
                    stale.provider.clone(),
                    stale.session.clone(),
                    delivery.submission.operation_id,
                ),
                presentation: PresentationKind::FinalAnswer,
                body: ContentText::new("project output").expect("output"),
            },
        )
        .expect("project output plan resolves");
        persistence
            .persist_output(
                stale.agent,
                &stale.provider,
                &stale.session,
                Some(&delivery),
                &output("project output"),
            )
            .expect("project-bound output");
        let activity = HarnessActivity {
            operation_id: delivery.submission.operation_id,
            item: None,
            kind: ActivityKind::Status,
            logical_key: ShortText::new("operation").expect("logical key"),
            runtime: ShortText::new("runtime").expect("runtime"),
            sequence: NonZeroU64::MIN,
            status: ActivityStatus::Succeeded,
            content: ContentText::new("complete").expect("activity"),
            truncated: false,
        };
        persistence
            .persist_activity(
                stale.agent,
                &stale.provider,
                &stale.session,
                Some(&delivery),
                &activity,
            )
            .expect("project-bound activity");
        let retained = ports.0.retained.lock().expect("retained lock");
        assert!(retained.values().any(|(_, payload)| matches!(
            payload,
            hq_domain::SemanticPayload::ProjectOutputRecorded {
                project_id: actual_project,
                dispatch_id: actual_dispatch,
                binding: actual_binding,
                thread_id: actual_thread,
                message,
                ..
            } if *actual_project == project_id
                && *actual_dispatch == dispatch_id
                && actual_binding.assignment_id == assignment_id
                && *actual_thread == thread_id
                && message.recipient == Some(MailboxAddress::new(home, MailboxId::from_bytes([19; 32])))
                && message.purpose == hq_domain::MessagePurpose::ProjectOutput
        )));
        assert!(retained.values().any(|(_, payload)| matches!(
            payload,
            hq_domain::SemanticPayload::HarnessActivityRecorded {
                project: Some(attribution),
                correlation,
                ..
            } if attribution.project_id == project_id
                && attribution.dispatch_id == dispatch_id
                && attribution.binding.assignment_id == assignment_id
                && attribution.thread_id == thread_id
                && correlation.operation() == delivery.submission.operation_id
        )));
    }
}
