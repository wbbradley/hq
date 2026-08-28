//! Transaction-consistent remote project control fact adapter.

use std::collections::BTreeSet;

use hq_application::{
    ApplicationError, ApplicationErrorCode, CommitFacts, DomainSnapshot, FactMutation, FactPlan,
    MutationAttempt, MutationDecision, MutationOutcome, ProjectCommandRequest, QueryDomain,
};
use hq_domain::{
    AccountId, AuthorityReference, AuthorityRole, BoundedSet, CausalReferences, CommandDigest,
    CommandId, DomainError, ErrorCategory, ErrorCode, FactId, FactScope, InstallationId,
    MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, OperationCorrelation, OperationId, ProjectId,
    ProviderId, ProviderSessionId, RemoteCommandResult, RuntimeObservation, SemanticPayload,
    Timestamp,
};
use hq_reducer::{
    AuthorityProjection, AuthorityProjectionKey, MembershipState, ProjectProjection,
    ProjectProjectionKey, RemoteCommandStage, RemoteCommandView,
};
use sha2::{Digest, Sha256};

use crate::{
    MAX_REMOTE_PROJECT_REPAIRS, RemoteProjectCommandPort, RemoteProjectCommandProgress,
    RemoteProjectCommandRecord, RemoteProjectFactOutcome, decode_project_command_action,
    encode_project_command_action, project_command_request_digest,
};

/// Remote-control adapter over authoritative query and semantic commit capabilities.
pub struct ApplicationRemoteProjectCommandPort<P> {
    ports: P,
    local_installation: InstallationId,
}

impl<P> ApplicationRemoteProjectCommandPort<P> {
    /// Owns the application capabilities used for serialized remote-control authoring.
    pub const fn new(ports: P, local_installation: InstallationId) -> Self {
        Self {
            ports,
            local_installation,
        }
    }

    /// Consumes the adapter and returns its capability bundle.
    pub fn into_ports(self) -> P {
        self.ports
    }
}

impl<P> RemoteProjectCommandPort for ApplicationRemoteProjectCommandPort<P>
where
    P: QueryDomain + CommitFacts,
{
    fn command(
        &self,
        command_id: CommandId,
    ) -> Result<Option<RemoteProjectCommandRecord>, ApplicationError> {
        let snapshot = self.ports.authoritative_snapshot()?;
        command_record(snapshot.domain(), command_id).transpose()
    }

    fn pending(
        &self,
        home: InstallationId,
        limit: usize,
    ) -> Result<Vec<RemoteProjectCommandRecord>, ApplicationError> {
        if limit == 0 || limit > MAX_REMOTE_PROJECT_REPAIRS {
            return Err(ApplicationError::new(ApplicationErrorCode::InvalidRequest));
        }
        let snapshot = self.ports.authoritative_snapshot()?;
        let mut commands = snapshot
            .domain()
            .project()
            .projections()
            .iter()
            .filter_map(|(key, projection)| match (key, projection) {
                (ProjectProjectionKey::Command(command_id), ProjectProjection::Command(view))
                    if view.target_home == home
                        && matches!(
                            view.stage,
                            RemoteCommandStage::Queued | RemoteCommandStage::Received { .. }
                        ) =>
                {
                    Some(command_record_from_view(*command_id, view))
                }
                _ => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        commands.sort_by_key(|record| (record.request.issued_at, record.request.command_id));
        commands.truncate(limit);
        Ok(commands)
    }

    fn author_request(
        &self,
        request: &ProjectCommandRequest,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError> {
        let body = encode_project_command_action(&request.action).map_err(codec_error)?;
        if project_command_request_digest(request).map_err(codec_error)? != request.request_digest {
            return Ok(RemoteProjectFactOutcome::Rejected(domain_error(
                ErrorCategory::InvalidInput,
                "project_command_digest_mismatch",
            )));
        }
        let mutation_input = request.clone();
        let requester = self.local_installation;
        let mutation_digest = remote_mutation_digest(
            request.command_id,
            b"request",
            &[request.request_digest.as_bytes(), body.as_str().as_bytes()],
        );
        let attempt = self.ports.commit_facts(FactMutation::new(
            remote_mutation_command(request.command_id, b"request"),
            mutation_digest,
            move |snapshot| match request_plan(snapshot, requester, &mutation_input, body.clone()) {
                Ok(plan) => MutationDecision::commit(plan),
                Err(error) => MutationDecision::reject(error),
            },
        ))?;
        Ok(remote_fact_outcome(attempt))
    }

    fn author_receipt(
        &self,
        command_id: CommandId,
        received_at: Timestamp,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError> {
        let home = self.local_installation;
        let mutation_digest = remote_mutation_digest(
            command_id,
            b"receipt",
            &[&received_at.as_unix_millis().to_be_bytes()],
        );
        let attempt = self.ports.commit_facts(FactMutation::new(
            remote_mutation_command(command_id, b"receipt"),
            mutation_digest,
            move |snapshot| match receipt_plan(snapshot, home, command_id, received_at) {
                Ok(plan) => MutationDecision::commit(plan),
                Err(error) => MutationDecision::reject(error),
            },
        ))?;
        Ok(remote_fact_outcome(attempt))
    }

    fn author_outcome(
        &self,
        command_id: CommandId,
        result: RemoteCommandResult,
        runtime: Option<RuntimeObservation>,
    ) -> Result<RemoteProjectFactOutcome, ApplicationError> {
        let home = self.local_installation;
        let result_digest = remote_result_digest(&result, runtime.as_ref());
        let mutation_digest =
            remote_mutation_digest(command_id, b"outcome", &[result_digest.as_bytes()]);
        let decision_result = result.clone();
        let decision_runtime = runtime.clone();
        let attempt = self.ports.commit_facts(FactMutation::new(
            remote_mutation_command(command_id, b"outcome"),
            mutation_digest,
            move |snapshot| match outcome_plan(
                snapshot,
                home,
                command_id,
                decision_result.clone(),
                decision_runtime.clone(),
            ) {
                Ok(plan) => MutationDecision::commit(plan),
                Err(error) => MutationDecision::reject(error),
            },
        ))?;
        Ok(remote_fact_outcome(attempt))
    }
}

fn request_plan(
    snapshot: &DomainSnapshot,
    requester: InstallationId,
    request: &ProjectCommandRequest,
    body: hq_domain::ContentText,
) -> Result<FactPlan, DomainError> {
    let Some(expected_head) = request.expected_head else {
        return Err(domain_error(
            ErrorCategory::InvalidInput,
            "remote_project_creation_requires_home",
        ));
    };
    let project = project_view(snapshot, request.project_id)?;
    if project.home != request.home {
        return Err(domain_error(
            ErrorCategory::Unauthorized,
            "project_wrong_home",
        ));
    }
    if project.head != expected_head {
        return Err(domain_error(ErrorCategory::Conflict, "project_stale_head"));
    }
    let active_human = active_human_authority(snapshot, request.account_id, requester)
        .ok_or_else(|| domain_error(ErrorCategory::Unauthorized, "project_inactive_human"))?;
    let mut parents = BTreeSet::from([project.head, active_human]);
    parents.extend(
        snapshot
            .project()
            .projections()
            .values()
            .filter_map(|projection| match projection {
                ProjectProjection::Command(command)
                    if command.project_id == request.project_id
                        && matches!(
                            command.stage,
                            RemoteCommandStage::Queued | RemoteCommandStage::Received { .. }
                        ) =>
                {
                    Some(command.request_fact)
                }
                _ => None,
            }),
    );
    let parents = bounded_parents(parents, "project_remote_parent_overflow")?;
    let causal = CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
        parents,
        [
            AuthorityReference::new(AuthorityRole::AccountMembership, active_human),
            AuthorityReference::new(AuthorityRole::ActiveHuman, active_human),
        ],
    )
    .map_err(|_| {
        domain_error(
            ErrorCategory::InvariantViolation,
            "project_remote_causal_invalid",
        )
    })?;
    Ok(FactPlan::new(
        requester,
        request.issued_at,
        FactScope::RemoteControl {
            account_id: request.account_id,
            target_home: request.home,
        },
        causal,
        SemanticPayload::RemoteProjectCommandRequested {
            command_id: request.command_id,
            digest: request.request_digest,
            project_id: request.project_id,
            target_home: request.home,
            expected_head,
            operation: routing_correlation(request.operation_id)?,
            body,
        },
        *request.request_digest.as_bytes(),
    ))
}

fn receipt_plan(
    snapshot: &DomainSnapshot,
    home: InstallationId,
    command_id: CommandId,
    received_at: Timestamp,
) -> Result<FactPlan, DomainError> {
    let command = command_view(snapshot, command_id)?;
    if command.target_home != home || !matches!(command.stage, RemoteCommandStage::Queued) {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "project_remote_invalid_receipt",
        ));
    }
    let project = project_view(snapshot, command.project_id)?;
    if project.home != home {
        return Err(domain_error(
            ErrorCategory::Unauthorized,
            "project_remote_wrong_home",
        ));
    }
    let root = installation_root(snapshot, home)?;
    let causal = project_home_causal(
        BTreeSet::from([command.request_fact, project.head, root]),
        root,
        command.request_fact,
    )?;
    Ok(FactPlan::new(
        home,
        received_at,
        FactScope::RemoteControl {
            account_id: command.account_id,
            target_home: home,
        },
        causal,
        SemanticPayload::RemoteProjectCommandReceipt {
            command_id,
            digest: command.digest,
            project_id: command.project_id,
            received_head: project.head,
            received_at,
        },
        *remote_mutation_digest(command_id, b"receipt-plan", &[project.head.as_bytes()]).as_bytes(),
    ))
}

fn outcome_plan(
    snapshot: &DomainSnapshot,
    home: InstallationId,
    command_id: CommandId,
    result: RemoteCommandResult,
    runtime: Option<RuntimeObservation>,
) -> Result<FactPlan, DomainError> {
    let command = command_view(snapshot, command_id)?;
    let RemoteCommandStage::Received {
        receipt_fact,
        received_at,
        ..
    } = command.stage
    else {
        return Err(domain_error(
            ErrorCategory::Conflict,
            "project_remote_invalid_outcome",
        ));
    };
    if command.target_home != home {
        return Err(domain_error(
            ErrorCategory::Unauthorized,
            "project_remote_wrong_home",
        ));
    }
    let project = project_view(snapshot, command.project_id)?;
    let mut parents = BTreeSet::from([command.request_fact, receipt_fact]);
    if let RemoteCommandResult::Committed(head) = result {
        if head != project.head {
            return Err(domain_error(
                ErrorCategory::Conflict,
                "project_remote_result_head_mismatch",
            ));
        }
        parents.insert(head);
    }
    let root = installation_root(snapshot, home)?;
    parents.insert(root);
    let causal = project_home_causal(parents, root, command.request_fact)?;
    Ok(FactPlan::new(
        home,
        received_at,
        FactScope::RemoteControl {
            account_id: command.account_id,
            target_home: home,
        },
        causal,
        SemanticPayload::RemoteProjectCommandOutcome {
            command_id,
            digest: command.digest,
            project_id: command.project_id,
            result,
            runtime,
        },
        *remote_mutation_digest(command_id, b"outcome-plan", &[receipt_fact.as_bytes()]).as_bytes(),
    ))
}

fn command_record(
    snapshot: &DomainSnapshot,
    command_id: CommandId,
) -> Option<Result<RemoteProjectCommandRecord, ApplicationError>> {
    match snapshot
        .project()
        .projection(ProjectProjectionKey::Command(command_id))
    {
        Some(ProjectProjection::Command(view)) => Some(command_record_from_view(command_id, view)),
        _ => None,
    }
}

fn command_record_from_view(
    command_id: CommandId,
    view: &RemoteCommandView,
) -> Result<RemoteProjectCommandRecord, ApplicationError> {
    let action = decode_project_command_action(&view.body).map_err(codec_error)?;
    let request = ProjectCommandRequest {
        command_id,
        operation_id: view.operation.operation(),
        request_digest: view.digest,
        account_id: view.account_id,
        project_id: view.project_id,
        home: view.target_home,
        expected_head: Some(view.expected_head),
        issued_at: view.issued_at,
        action,
    };
    if project_command_request_digest(&request).map_err(codec_error)? != view.digest {
        return Err(ApplicationError::new(ApplicationErrorCode::StateCorrupt));
    }
    let progress = match &view.stage {
        RemoteCommandStage::Queued => RemoteProjectCommandProgress::Queued,
        RemoteCommandStage::Received {
            receipt_fact,
            received_head,
            received_at,
        } => RemoteProjectCommandProgress::Received {
            receipt_fact: *receipt_fact,
            received_head: *received_head,
            received_at: *received_at,
        },
        RemoteCommandStage::Terminal {
            receipt_fact,
            received_head,
            received_at,
            outcome_fact,
            result,
            runtime,
        } => RemoteProjectCommandProgress::Terminal {
            receipt_fact: *receipt_fact,
            received_head: *received_head,
            received_at: *received_at,
            outcome_fact: *outcome_fact,
            result: result.clone(),
            runtime: runtime.clone(),
        },
        RemoteCommandStage::Conflicted => RemoteProjectCommandProgress::Conflicted,
    };
    Ok(RemoteProjectCommandRecord {
        request,
        request_fact: view.request_fact,
        progress,
    })
}

fn project_view(
    snapshot: &DomainSnapshot,
    project_id: ProjectId,
) -> Result<&hq_reducer::ProjectView, DomainError> {
    match snapshot
        .project()
        .projection(ProjectProjectionKey::Project(project_id))
    {
        Some(ProjectProjection::Project(view)) => Ok(view),
        _ => Err(domain_error(ErrorCategory::NotFound, "project_not_found")),
    }
}

fn command_view(
    snapshot: &DomainSnapshot,
    command_id: CommandId,
) -> Result<&RemoteCommandView, DomainError> {
    match snapshot
        .project()
        .projection(ProjectProjectionKey::Command(command_id))
    {
        Some(ProjectProjection::Command(view)) => Ok(view),
        _ => Err(domain_error(
            ErrorCategory::NotFound,
            "project_remote_command_missing",
        )),
    }
}

fn active_human_authority(
    snapshot: &DomainSnapshot,
    account_id: AccountId,
    installation: InstallationId,
) -> Option<FactId> {
    if let Some(AuthorityProjection::Account {
        root_fact, creator, ..
    }) = snapshot
        .authority()
        .projection(AuthorityProjectionKey::Account(account_id))
        && creator.installation_id() == installation
    {
        return Some(*root_fact);
    }
    match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Membership {
            account: account_id,
            device: installation,
        }) {
        Some(AuthorityProjection::Membership(membership))
            if membership.state() == MembershipState::Active =>
        {
            membership.active_acceptances.iter().next().copied()
        }
        _ => None,
    }
}

fn installation_root(
    snapshot: &DomainSnapshot,
    home: InstallationId,
) -> Result<FactId, DomainError> {
    match snapshot
        .authority()
        .projection(AuthorityProjectionKey::Installation(home))
    {
        Some(AuthorityProjection::Installation(installation)) => Ok(installation.root_fact),
        _ => Err(domain_error(
            ErrorCategory::Unauthorized,
            "project_home_missing",
        )),
    }
}

fn project_home_causal(
    parents: BTreeSet<FactId>,
    root: FactId,
    request: FactId,
) -> Result<CausalReferences<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>, DomainError> {
    CausalReferences::new(
        bounded_parents(parents, "project_remote_parent_overflow")?,
        [
            AuthorityReference::new(AuthorityRole::ProjectHome, root),
            AuthorityReference::new(AuthorityRole::Request, request),
        ],
    )
    .map_err(|_| {
        domain_error(
            ErrorCategory::InvariantViolation,
            "project_remote_causal_invalid",
        )
    })
}

fn bounded_parents(
    parents: BTreeSet<FactId>,
    code: &str,
) -> Result<BoundedSet<FactId, MAX_FACT_PARENTS>, DomainError> {
    BoundedSet::new(parents).map_err(|_| domain_error(ErrorCategory::InvariantViolation, code))
}

fn routing_correlation(operation_id: OperationId) -> Result<OperationCorrelation, DomainError> {
    Ok(OperationCorrelation::new(
        ProviderId::new("hq").map_err(|_| {
            domain_error(
                ErrorCategory::InvariantViolation,
                "project_remote_provider_invalid",
            )
        })?,
        ProviderSessionId::new("project-control-v1").map_err(|_| {
            domain_error(
                ErrorCategory::InvariantViolation,
                "project_remote_session_invalid",
            )
        })?,
        operation_id,
    ))
}

fn remote_fact_outcome(attempt: MutationAttempt) -> RemoteProjectFactOutcome {
    match attempt {
        MutationAttempt::Uncertain { .. } => RemoteProjectFactOutcome::Uncertain,
        MutationAttempt::Completed(receipt) => match receipt.outcome() {
            MutationOutcome::Committed => RemoteProjectFactOutcome::Committed,
            MutationOutcome::Rejected(error) => RemoteProjectFactOutcome::Rejected(error.clone()),
        },
    }
}

fn remote_mutation_command(command_id: CommandId, tag: &[u8]) -> CommandId {
    CommandId::from_bytes(hash(&[
        b"hq-project-remote-fact-command-v1",
        command_id.as_bytes(),
        tag,
    ]))
}

fn remote_mutation_digest(command_id: CommandId, tag: &[u8], parts: &[&[u8]]) -> CommandDigest {
    let mut values = vec![
        b"hq-project-remote-fact-digest-v1".as_slice(),
        command_id.as_bytes().as_slice(),
        tag,
    ];
    values.extend_from_slice(parts);
    CommandDigest::from_bytes(hash(&values))
}

fn remote_result_digest(
    result: &RemoteCommandResult,
    runtime: Option<&RuntimeObservation>,
) -> CommandDigest {
    let mut digest = Sha256::new();
    digest.update(b"hq-project-remote-result-v1\0");
    match result {
        RemoteCommandResult::Committed(head) => {
            digest.update([1]);
            digest.update(head.as_bytes());
        }
        RemoteCommandResult::Rejected(error) => {
            digest.update([2]);
            put(&mut digest, error.as_str().as_bytes());
        }
    }
    match runtime {
        None => digest.update([0]),
        Some(RuntimeObservation::Succeeded) => digest.update([1]),
        Some(RuntimeObservation::Failed(error)) => {
            digest.update([2]);
            put(&mut digest, error.as_str().as_bytes());
        }
        Some(RuntimeObservation::Uncertain(error)) => {
            digest.update([3]);
            put(&mut digest, error.as_str().as_bytes());
        }
    }
    CommandDigest::from_bytes(digest.finalize().into())
}

fn hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in parts {
        put(&mut digest, part);
    }
    digest.finalize().into()
}

fn put(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

fn domain_error(category: ErrorCategory, code: &str) -> DomainError {
    let code = ErrorCode::new(code)
        .unwrap_or_else(|_| unreachable!("static project error codes are valid"));
    DomainError::new(category, code)
}

fn codec_error(_error: crate::ProjectCommandCodecError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorCode::StateCorrupt)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::collections::{BTreeMap, BTreeSet};

    use hq_application::ProjectionSnapshot;
    use hq_domain::{
        BoundedText, EncryptionPublicKey, InstallationAddress, MailboxAddress, MailboxId,
        ResourceLocator, ResourceScheme, ShortText, SigningPublicKey,
    };
    use hq_reducer::{InstallationView, ProjectLifecycle, ProjectView};

    use super::*;
    use crate::ProjectCommandAction;

    #[test]
    fn project_creation_is_never_routed_to_a_non_home_installation() {
        let fixture = Fixture::new();
        let snapshot = fixture.snapshot(RemoteCommandStage::Queued);
        let mut request = fixture.request();
        request.expected_head = None;
        request.action =
            ProjectCommandAction::ProvisionWorktree(hq_application::WorktreeProvisioningRequest {
                mailbox_id: MailboxId::from_bytes([90; 32]),
                project_name: ShortText::new("created").expect("name"),
                brief: None,
                source: ResourceLocator::new(
                    ResourceScheme::WorkingTree,
                    BoundedText::new("/repo").expect("path"),
                ),
                destination: ResourceLocator::new(
                    ResourceScheme::WorkingTree,
                    BoundedText::new("/repo/worktree").expect("path"),
                ),
                branch: ShortText::new("feature").expect("branch"),
                create_branch: true,
            });
        request.request_digest = project_command_request_digest(&request).expect("request digest");
        let body = encode_project_command_action(&request.action).expect("action encodes");

        let error = request_plan(&snapshot, fixture.requester, &request, body)
            .expect_err("creation must execute at its home");
        assert_eq!(
            error.code().as_str(),
            "remote_project_creation_requires_home"
        );
    }

    #[test]
    fn plans_bind_request_receipt_and_outcome_to_exact_authority_and_heads() {
        let fixture = Fixture::new();
        let queued = fixture.snapshot(RemoteCommandStage::Queued);
        let request = fixture.request();

        let request_fact = request_plan(
            &queued,
            fixture.requester,
            &request,
            encode_project_command_action(&request.action).expect("action encodes"),
        )
        .expect("request plan");
        assert_eq!(
            request_fact.scope(),
            &FactScope::RemoteControl {
                account_id: fixture.account,
                target_home: fixture.home,
            }
        );
        assert!(request_fact.causal().parents().contains(&fixture.head));
        assert!(
            request_fact
                .causal()
                .parents()
                .contains(&fixture.account_root)
        );

        let receipt = receipt_plan(&queued, fixture.home, fixture.command, fixture.received_at)
            .expect("receipt plan");
        assert_eq!(
            receipt.causal().parents(),
            &BoundedSet::new([fixture.request_fact, fixture.head, fixture.home_root])
                .expect("bounded parents")
        );
        assert_eq!(
            receipt.causal().authority(AuthorityRole::Request),
            Some(fixture.request_fact)
        );
        assert_eq!(
            receipt.causal().authority(AuthorityRole::ProjectHome),
            Some(fixture.home_root)
        );

        let received = fixture.snapshot(RemoteCommandStage::Received {
            receipt_fact: fixture.receipt_fact,
            received_head: fixture.head,
            received_at: fixture.received_at,
        });
        let outcome = outcome_plan(
            &received,
            fixture.home,
            fixture.command,
            RemoteCommandResult::Committed(fixture.head),
            Some(RuntimeObservation::Succeeded),
        )
        .expect("outcome plan");
        for parent in [
            fixture.request_fact,
            fixture.receipt_fact,
            fixture.head,
            fixture.home_root,
        ] {
            assert!(outcome.causal().parents().contains(&parent));
        }
    }

    #[test]
    fn plans_reject_stale_request_and_changed_committed_result() {
        let fixture = Fixture::new();
        let queued = fixture.snapshot(RemoteCommandStage::Queued);
        let mut request = fixture.request();
        request.expected_head = Some(FactId::from_bytes([90; 32]));
        let error = request_plan(
            &queued,
            fixture.requester,
            &request,
            encode_project_command_action(&request.action).expect("action encodes"),
        )
        .expect_err("stale request");
        assert_eq!(error.code().as_str(), "project_stale_head");

        let received = fixture.snapshot(RemoteCommandStage::Received {
            receipt_fact: fixture.receipt_fact,
            received_head: fixture.head,
            received_at: fixture.received_at,
        });
        let error = outcome_plan(
            &received,
            fixture.home,
            fixture.command,
            RemoteCommandResult::Committed(FactId::from_bytes([91; 32])),
            None,
        )
        .expect_err("changed head");
        assert_eq!(error.code().as_str(), "project_remote_result_head_mismatch");
    }

    struct Fixture {
        requester: InstallationId,
        home: InstallationId,
        account: AccountId,
        project: ProjectId,
        command: CommandId,
        head: FactId,
        account_root: FactId,
        home_root: FactId,
        request_fact: FactId,
        receipt_fact: FactId,
        received_at: Timestamp,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                requester: InstallationId::from_bytes([1; 32]),
                home: InstallationId::from_bytes([2; 32]),
                account: AccountId::from_bytes([3; 32]),
                project: ProjectId::from_bytes([4; 32]),
                command: CommandId::from_bytes([5; 32]),
                head: FactId::from_bytes([6; 32]),
                account_root: FactId::from_bytes([7; 32]),
                home_root: FactId::from_bytes([8; 32]),
                request_fact: FactId::from_bytes([9; 32]),
                receipt_fact: FactId::from_bytes([10; 32]),
                received_at: Timestamp::from_unix_millis(12),
            }
        }

        fn request(&self) -> ProjectCommandRequest {
            let mut request = ProjectCommandRequest {
                command_id: self.command,
                operation_id: OperationId::from_bytes([11; 32]),
                request_digest: CommandDigest::from_bytes([0; 32]),
                account_id: self.account,
                project_id: self.project,
                home: self.home,
                expected_head: Some(self.head),
                issued_at: Timestamp::from_unix_millis(13),
                action: ProjectCommandAction::Open,
            };
            request.request_digest =
                project_command_request_digest(&request).expect("request digest");
            request
        }

        fn snapshot(&self, stage: RemoteCommandStage) -> DomainSnapshot {
            let request = self.request();
            let body = encode_project_command_action(&request.action).expect("action encodes");
            let project = ProjectView {
                root: FactId::from_bytes([14; 32]),
                head: self.head,
                fork_participants: BTreeSet::new(),
                home: self.home,
                mailbox: MailboxAddress::new(self.home, MailboxId::from_bytes([15; 32])),
                predecessor: None,
                name: ShortText::new("project").expect("name"),
                brief: None,
                resources: BTreeMap::new(),
                primary: None,
                lifecycle: ProjectLifecycle::Open,
                archived: false,
                active_claims: BTreeSet::new(),
                claim_conflicts: BTreeMap::new(),
                claimable: true,
                assignment: None,
                input_sequence: 0,
            };
            let command = RemoteCommandView {
                account_id: self.account,
                digest: request.request_digest,
                project_id: self.project,
                expected_head: self.head,
                target_home: self.home,
                operation: routing_correlation(request.operation_id).expect("correlation"),
                body,
                issued_at: request.issued_at,
                request_fact: self.request_fact,
                stage,
                support: BTreeSet::from([self.request_fact]),
            };
            DomainSnapshot::new(
                ProjectionSnapshot::new(
                    BTreeMap::new(),
                    BTreeMap::from([
                        (
                            AuthorityProjectionKey::Installation(self.home),
                            AuthorityProjection::Installation(InstallationView {
                                root_fact: self.home_root,
                                signing_key: SigningPublicKey::from_bytes([16; 32]),
                                encryption_key: EncryptionPublicKey::from_bytes([17; 32]),
                                label: None,
                            }),
                        ),
                        (
                            AuthorityProjectionKey::Account(self.account),
                            AuthorityProjection::Account {
                                root_fact: self.account_root,
                                creator: InstallationAddress::new(
                                    self.requester,
                                    SigningPublicKey::from_bytes([18; 32]),
                                ),
                                label: None,
                            },
                        ),
                    ]),
                    BTreeMap::new(),
                ),
                ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
                ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
                ProjectionSnapshot::new(
                    BTreeMap::new(),
                    BTreeMap::from([
                        (
                            ProjectProjectionKey::Project(self.project),
                            ProjectProjection::Project(Box::new(project)),
                        ),
                        (
                            ProjectProjectionKey::Command(self.command),
                            ProjectProjection::Command(Box::new(command)),
                        ),
                    ]),
                    BTreeMap::new(),
                ),
            )
        }
    }
}
