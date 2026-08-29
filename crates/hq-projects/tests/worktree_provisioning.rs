//! Recoverable Git worktree provisioning contracts.

#![allow(clippy::expect_used, clippy::panic)]

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use hq_application::{
    ApplicationError, ApplicationErrorCode, EffectOutcome, EffectRequest, ProjectCommandAction,
    ProjectCommandOutcome, ProjectCommandRequest, ProjectCreationRequest,
    WorktreeProvisioningRequest,
};
use hq_domain::{
    AccountId, BoundedText, CommandDigest, CommandId, DomainError, ErrorCategory, ErrorCode,
    FactId, InstallationId, MailboxId, OperationId, ProjectExternalStateWarning, ProjectId,
    ProjectResource, ResourceHealth, ResourceId, ResourceLocator, ResourceScheme, ShortText,
    Timestamp,
};
use hq_projects::{
    BeginSagaOutcome, CanonicalProjectMutation, CanonicalProjectMutationAction,
    CanonicalProjectMutationOutcome, CanonicalProjectPort, GitWorktreeAdapter,
    GitWorktreeAdapterConfig, GitWorktreePort, GitWorktreeRequest, GitWorktreeState,
    ProjectLaunchObservation, ProjectLaunchValidationRequest, ProjectReleaseAssessmentRequest,
    ProjectResourceIdentificationRequest, ProjectResourceObservation, ProjectResourcePort,
    ProjectResourceValidationRequest, ProjectRuntimeDelivery, ProjectRuntimePort,
    ProjectRuntimeRequest, ProjectSagaRecord, ProjectSagaStore, SagaStoreError,
    project_command_request_digest,
};
use hq_resources::{ExecGit, GitCommandConfig, PathReleaseAssessment};

static NEXT_GIT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Default)]
struct MemoryStore(Arc<Mutex<BTreeMap<OperationId, ProjectSagaRecord>>>);

impl ProjectSagaStore for MemoryStore {
    fn find(&self, operation_id: OperationId) -> Result<Option<ProjectSagaRecord>, SagaStoreError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| SagaStoreError::Unavailable)?
            .values()
            .find(|record| record.operation_id == operation_id)
            .cloned())
    }

    fn begin(&self, record: ProjectSagaRecord) -> Result<BeginSagaOutcome, SagaStoreError> {
        let mut records = self.0.lock().expect("store lock");
        if let Some(existing) = records.get(&record.operation_id) {
            return Ok(
                if existing.command_id == record.command_id
                    && existing.request_digest == record.request_digest
                {
                    BeginSagaOutcome::Existing(existing.clone())
                } else {
                    BeginSagaOutcome::IdentityConflict
                },
            );
        }
        records.insert(record.operation_id, record.clone());
        Ok(BeginSagaOutcome::Inserted(record))
    }

    fn replace(&self, record: ProjectSagaRecord) -> Result<(), SagaStoreError> {
        self.0
            .lock()
            .expect("store lock")
            .insert(record.operation_id, record);
        Ok(())
    }

    fn runnable(&self, limit: usize) -> Result<Vec<ProjectSagaRecord>, SagaStoreError> {
        Ok(self
            .0
            .lock()
            .expect("store lock")
            .values()
            .filter(|record| !record.state.is_terminal())
            .take(limit)
            .cloned()
            .collect())
    }
}

#[derive(Clone)]
struct ScriptedCanonical {
    mutations: Arc<Mutex<Vec<CanonicalProjectMutation>>>,
    outcomes: Arc<Mutex<VecDeque<CanonicalProjectMutationOutcome>>>,
}

impl ScriptedCanonical {
    fn committing() -> Self {
        Self::with_outcomes([CanonicalProjectMutationOutcome::Committed {
            project_head: FactId::from_bytes([99; 32]),
        }])
    }

    fn with_outcomes(outcomes: impl IntoIterator<Item = CanonicalProjectMutationOutcome>) -> Self {
        Self {
            mutations: Arc::new(Mutex::new(Vec::new())),
            outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
        }
    }
}

impl CanonicalProjectPort for ScriptedCanonical {
    fn snapshot(
        &self,
        _project_id: ProjectId,
        _account_id: AccountId,
        _requested_agent: Option<hq_domain::AgentId>,
    ) -> Result<hq_projects::ProjectWorkflowSnapshot, ApplicationError> {
        Err(ApplicationError::new(ApplicationErrorCode::ItemNotFound))
    }

    fn mutate(
        &self,
        mutation: CanonicalProjectMutation,
    ) -> Result<CanonicalProjectMutationOutcome, ApplicationError> {
        self.mutations.lock().expect("mutation lock").push(mutation);
        Ok(self
            .outcomes
            .lock()
            .expect("outcome lock")
            .pop_front()
            .unwrap_or(CanonicalProjectMutationOutcome::Committed {
                project_head: FactId::from_bytes([99; 32]),
            }))
    }
}

#[derive(Clone)]
struct ScriptedGit {
    lookups: Arc<Mutex<VecDeque<EffectOutcome<GitWorktreeState>>>>,
    creates: Arc<Mutex<VecDeque<EffectOutcome<()>>>>,
    lookup_calls: Arc<Mutex<Vec<EffectRequest<GitWorktreeRequest>>>>,
    create_calls: Arc<Mutex<Vec<EffectRequest<GitWorktreeRequest>>>>,
}

impl ScriptedGit {
    fn new(
        lookups: impl IntoIterator<Item = EffectOutcome<GitWorktreeState>>,
        creates: impl IntoIterator<Item = EffectOutcome<()>>,
    ) -> Self {
        Self {
            lookups: Arc::new(Mutex::new(lookups.into_iter().collect())),
            creates: Arc::new(Mutex::new(creates.into_iter().collect())),
            lookup_calls: Arc::new(Mutex::new(Vec::new())),
            create_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl GitWorktreePort for ScriptedGit {
    fn lookup(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<GitWorktreeState>, ApplicationError> {
        self.lookup_calls
            .lock()
            .expect("lookup calls")
            .push(request.clone());
        self.lookups
            .lock()
            .expect("lookups")
            .pop_front()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))
    }

    fn create(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.create_calls
            .lock()
            .expect("create calls")
            .push(request.clone());
        self.creates
            .lock()
            .expect("creates")
            .pop_front()
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))
    }
}

struct LoseFirstAcceptedCreate<G> {
    inner: Arc<G>,
    lose_response: Arc<AtomicBool>,
    create_calls: Arc<AtomicU64>,
}

impl<G> Clone for LoseFirstAcceptedCreate<G> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            lose_response: Arc::clone(&self.lose_response),
            create_calls: Arc::clone(&self.create_calls),
        }
    }
}

impl<G: GitWorktreePort> GitWorktreePort for LoseFirstAcceptedCreate<G> {
    fn lookup(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<GitWorktreeState>, ApplicationError> {
        self.inner.lookup(request)
    }

    fn create(
        &self,
        request: &EffectRequest<GitWorktreeRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        self.create_calls.fetch_add(1, Ordering::Relaxed);
        let outcome = self.inner.create(request)?;
        if matches!(outcome, EffectOutcome::Accepted(()))
            && self.lose_response.swap(false, Ordering::AcqRel)
        {
            Ok(EffectOutcome::Uncertain(request.operation_id))
        } else {
            Ok(outcome)
        }
    }
}

struct GitDirectory(PathBuf);

impl GitDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hq-project-workflow-git-{}-{}",
            std::process::id(),
            NEXT_GIT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("Git fixture directory creates");
        Self(path)
    }

    fn join(&self, child: &str) -> PathBuf {
        self.0.join(child)
    }
}

impl Drop for GitDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
struct IdentifiedResource(ProjectResource);

impl ProjectResourcePort for IdentifiedResource {
    fn identify_resource(
        &self,
        _request: &EffectRequest<ProjectResourceIdentificationRequest>,
    ) -> Result<EffectOutcome<ProjectResource>, ApplicationError> {
        Ok(EffectOutcome::Accepted(self.0.clone()))
    }

    fn validate_resources(
        &self,
        _request: &EffectRequest<ProjectResourceValidationRequest>,
    ) -> Result<EffectOutcome<Vec<ProjectResourceObservation>>, ApplicationError> {
        unavailable()
    }

    fn assess_release(
        &self,
        _request: &EffectRequest<ProjectReleaseAssessmentRequest>,
    ) -> Result<EffectOutcome<Vec<PathReleaseAssessment>>, ApplicationError> {
        unavailable()
    }

    fn validate_launch_directory(
        &self,
        _request: &EffectRequest<ProjectLaunchValidationRequest>,
    ) -> Result<EffectOutcome<ProjectLaunchObservation>, ApplicationError> {
        unavailable()
    }
}

#[test]
fn existing_resource_creation_identifies_then_replays_exact_no_head_mutation() {
    let request = existing_creation_request();
    let ProjectCommandAction::Create(creation) = &request.action else {
        panic!("creation request")
    };
    let resource = ProjectResource {
        resource_id: creation.resource_id,
        display_locator: creation.resource.clone(),
        canonical_locator: locator("/private/var/work/existing"),
        health: ResourceHealth::Healthy,
    };
    let canonical = ScriptedCanonical::with_outcomes([
        CanonicalProjectMutationOutcome::Uncertain,
        CanonicalProjectMutationOutcome::Committed {
            project_head: FactId::from_bytes([99; 32]),
        },
    ]);
    let manager = hq_projects::ProjectWorkflowManager::new(
        MemoryStore::default(),
        canonical.clone(),
        UnusedRuntime,
        IdentifiedResource(resource.clone()),
    );

    assert!(matches!(
        manager.control(request.clone()).expect("uncertain create"),
        ProjectCommandOutcome::Reconcilable { .. }
    ));
    assert_eq!(
        manager.control(request.clone()).expect("replayed create"),
        ProjectCommandOutcome::Completed {
            operation_id: request.operation_id,
            project_head: FactId::from_bytes([99; 32]),
            runtime: None,
        }
    );
    let mutations = canonical.mutations.lock().expect("mutations");
    assert_eq!(mutations.len(), 2);
    assert_eq!(mutations[0], mutations[1]);
    assert_eq!(mutations[0].expected_head, None);
    assert!(matches!(
        &mutations[0].action,
        CanonicalProjectMutationAction::Create {
            mailbox_id,
            name,
            resource: created,
            ..
        } if *mailbox_id == creation.mailbox_id
            && name == &creation.project_name
            && created == &resource
    ));
}

#[derive(Clone, Copy)]
struct UnusedRuntime;

impl ProjectRuntimePort for UnusedRuntime {
    fn start_or_resume(
        &self,
        _request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<hq_domain::ProviderSessionId>, ApplicationError> {
        unavailable()
    }

    fn deliver(
        &self,
        _request: &EffectRequest<ProjectRuntimeDelivery>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        unavailable()
    }

    fn stop(
        &self,
        _request: &EffectRequest<ProjectRuntimeRequest>,
    ) -> Result<EffectOutcome<()>, ApplicationError> {
        unavailable()
    }
}

#[test]
fn provisioning_checkpoints_git_identification_then_one_canonical_creation() {
    let request = provisioning_request();
    let resource = identified_resource(&request);
    let store = MemoryStore::default();
    let canonical = ScriptedCanonical::committing();
    let git = ScriptedGit::new(
        [EffectOutcome::Accepted(GitWorktreeState::ReadyToCreate)],
        [EffectOutcome::Accepted(())],
    );
    let manager = hq_projects::ProjectWorkflowManager::with_git(
        store,
        canonical.clone(),
        UnusedRuntime,
        IdentifiedResource(resource.clone()),
        git.clone(),
    );

    let outcome = manager.control(request.clone()).expect("workflow runs");
    assert_eq!(
        outcome,
        ProjectCommandOutcome::Completed {
            operation_id: request.operation_id,
            project_head: FactId::from_bytes([99; 32]),
            runtime: None,
        }
    );
    assert_eq!(git.lookup_calls.lock().expect("lookup calls").len(), 1);
    assert_eq!(git.create_calls.lock().expect("create calls").len(), 1);
    let mutations = canonical.mutations.lock().expect("mutations");
    assert_eq!(mutations.len(), 1);
    assert_eq!(mutations[0].expected_head, None);
    assert!(matches!(
        &mutations[0].action,
        CanonicalProjectMutationAction::Create {
            mailbox_id,
            name,
            resource: created,
            ..
        } if *mailbox_id == MailboxId::from_bytes([7; 32])
            && name.as_str() == "provisioned"
            && created == &resource
    ));
}

#[test]
fn lost_create_response_repairs_by_lookup_without_a_second_git_mutation() {
    let request = provisioning_request();
    let operation_id = derived_git_operation(&request);
    let git = ScriptedGit::new(
        [
            EffectOutcome::Accepted(GitWorktreeState::ReadyToCreate),
            EffectOutcome::Accepted(GitWorktreeState::Created),
        ],
        [EffectOutcome::Uncertain(operation_id)],
    );
    let manager = hq_projects::ProjectWorkflowManager::with_git(
        MemoryStore::default(),
        ScriptedCanonical::committing(),
        UnusedRuntime,
        IdentifiedResource(identified_resource(&request)),
        git.clone(),
    );

    let first = manager.control(request.clone()).expect("first attempt");
    assert!(matches!(
        first,
        ProjectCommandOutcome::Reconcilable {
            external_state_warning: Some(ProjectExternalStateWarning::WorktreeMayExist {
                destination,
                branch,
            }),
            ..
        } if destination.value() == "/tmp/hq-destination"
            && branch.as_str() == "feature/exact"
    ));
    assert!(matches!(
        manager.control(request).expect("repair"),
        ProjectCommandOutcome::Completed { .. }
    ));
    assert_eq!(git.lookup_calls.lock().expect("lookup calls").len(), 2);
    assert_eq!(git.create_calls.lock().expect("create calls").len(), 1);
}

#[test]
fn real_git_response_loss_repairs_after_manager_restart_without_a_second_mutation() {
    let directory = GitDirectory::new();
    let repository = directory.join("repository");
    let destination = directory.join("worktree");
    initialize_git_repository(&repository);
    let mut request = provisioning_request();
    let ProjectCommandAction::ProvisionWorktree(provisioning) = &mut request.action else {
        panic!("provisioning request")
    };
    provisioning.source = path_locator(&repository);
    provisioning.destination = path_locator(&destination);
    provisioning.base = Some(ShortText::new("HEAD").expect("base"));
    request.request_digest = project_command_request_digest(&request).expect("digest");

    let real_git = GitWorktreeAdapter::new(
        GitWorktreeAdapterConfig {
            max_repository_locks: NonZeroUsize::new(4).expect("positive lock bound"),
        },
        ExecGit::new(GitCommandConfig {
            executable: PathBuf::from("git"),
            timeout: Duration::from_secs(5),
            max_output_bytes: 1024 * 1024,
        })
        .expect("bounded Git runner"),
    );
    let git = LoseFirstAcceptedCreate {
        inner: Arc::new(real_git),
        lose_response: Arc::new(AtomicBool::new(true)),
        create_calls: Arc::new(AtomicU64::new(0)),
    };
    let store = MemoryStore::default();
    let canonical = ScriptedCanonical::committing();

    let first = hq_projects::ProjectWorkflowManager::with_git(
        store.clone(),
        canonical.clone(),
        UnusedRuntime,
        IdentifiedResource(identified_resource(&request)),
        git.clone(),
    )
    .control(request.clone())
    .expect("first manager reports response loss");
    assert!(matches!(
        first,
        ProjectCommandOutcome::Reconcilable {
            external_state_warning: Some(ProjectExternalStateWarning::WorktreeMayExist { .. }),
            ..
        }
    ));
    assert!(destination.join(".git").is_file());

    let repaired = hq_projects::ProjectWorkflowManager::with_git(
        store,
        canonical,
        UnusedRuntime,
        IdentifiedResource(identified_resource(&request)),
        git.clone(),
    )
    .control(request)
    .expect("restarted manager reconciles exact Git state");
    assert!(matches!(repaired, ProjectCommandOutcome::Completed { .. }));
    assert_eq!(git.create_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn post_git_canonical_rejection_warns_without_removing_external_state() {
    let request = provisioning_request();
    let git = ScriptedGit::new(
        [EffectOutcome::Accepted(GitWorktreeState::ReadyToCreate)],
        [EffectOutcome::Accepted(())],
    );
    let canonical = ScriptedCanonical::with_outcomes([CanonicalProjectMutationOutcome::Rejected(
        domain_error(ErrorCategory::Conflict, "project_create_conflict"),
    )]);
    let outcome = hq_projects::ProjectWorkflowManager::with_git(
        MemoryStore::default(),
        canonical,
        UnusedRuntime,
        IdentifiedResource(identified_resource(&request)),
        git,
    )
    .control(request)
    .expect("definite canonical rejection");

    assert!(matches!(
        outcome,
        ProjectCommandOutcome::Rejected {
            external_state_warning: Some(ProjectExternalStateWarning::WorktreeMayExist {
                destination,
                branch,
            }),
            ..
        } if destination.value() == "/tmp/hq-destination"
            && branch.as_str() == "feature/exact"
    ));
}

#[test]
fn changed_or_missing_head_preconditions_fail_before_any_external_effect() {
    let mut provisioning = provisioning_request();
    provisioning.expected_head = Some(FactId::from_bytes([44; 32]));
    let git = ScriptedGit::new([], []);
    let manager = hq_projects::ProjectWorkflowManager::with_git(
        MemoryStore::default(),
        ScriptedCanonical::committing(),
        UnusedRuntime,
        IdentifiedResource(identified_resource(&provisioning)),
        git.clone(),
    );
    assert!(matches!(
        manager.control(provisioning).expect("definite rejection"),
        ProjectCommandOutcome::Rejected { .. }
    ));

    let mut existing = provisioning_request();
    existing.action = ProjectCommandAction::Open;
    existing.request_digest = project_command_request_digest(&existing).expect("digest");
    assert!(matches!(
        manager.control(existing).expect("definite rejection"),
        ProjectCommandOutcome::Rejected { .. }
    ));
    assert!(git.lookup_calls.lock().expect("lookup calls").is_empty());
    assert!(git.create_calls.lock().expect("create calls").is_empty());
}

#[test]
fn non_normalized_destination_is_rejected_before_any_external_effect() {
    let mut request = provisioning_request();
    let ProjectCommandAction::ProvisionWorktree(provisioning) = &mut request.action else {
        panic!("provisioning request")
    };
    provisioning.destination = locator("/tmp/hq-parent/../hq-destination");
    request.request_digest = project_command_request_digest(&request).expect("digest");
    let git = ScriptedGit::new([], []);
    let manager = hq_projects::ProjectWorkflowManager::with_git(
        MemoryStore::default(),
        ScriptedCanonical::committing(),
        UnusedRuntime,
        IdentifiedResource(identified_resource(&request)),
        git.clone(),
    );

    assert!(matches!(
        manager.control(request).expect("definite rejection"),
        ProjectCommandOutcome::Rejected { .. }
    ));
    assert!(git.lookup_calls.lock().expect("lookup calls").is_empty());
    assert!(git.create_calls.lock().expect("create calls").is_empty());
}

#[test]
fn branch_creation_mode_must_agree_with_the_presence_of_an_exact_base() {
    for (base, create_branch) in [(None, true), (Some("main"), false)] {
        let mut request = provisioning_request();
        let ProjectCommandAction::ProvisionWorktree(provisioning) = &mut request.action else {
            panic!("provisioning request")
        };
        provisioning.base = base.map(|value| ShortText::new(value).expect("base"));
        provisioning.create_branch = create_branch;
        request.request_digest = project_command_request_digest(&request).expect("digest");
        let git = ScriptedGit::new([], []);
        let manager = hq_projects::ProjectWorkflowManager::with_git(
            MemoryStore::default(),
            ScriptedCanonical::committing(),
            UnusedRuntime,
            IdentifiedResource(identified_resource(&request)),
            git.clone(),
        );

        assert!(matches!(
            manager.control(request).expect("definite rejection"),
            ProjectCommandOutcome::Rejected {
                error,
                external_state_warning: None,
                ..
            } if error.code().as_str() == "project_worktree_branch_mode_invalid"
        ));
        assert!(git.lookup_calls.lock().expect("lookup calls").is_empty());
        assert!(git.create_calls.lock().expect("create calls").is_empty());
    }
}

fn provisioning_request() -> ProjectCommandRequest {
    let source = locator("/tmp/hq-source");
    let destination = locator("/tmp/hq-destination");
    let mut request = ProjectCommandRequest {
        command_id: CommandId::from_bytes([1; 32]),
        operation_id: OperationId::from_bytes([2; 32]),
        request_digest: CommandDigest::from_bytes([0; 32]),
        account_id: AccountId::from_bytes([3; 32]),
        project_id: ProjectId::from_bytes([4; 32]),
        home: InstallationId::from_bytes([5; 32]),
        expected_head: None,
        issued_at: Timestamp::from_unix_millis(6),
        action: ProjectCommandAction::ProvisionWorktree(WorktreeProvisioningRequest {
            mailbox_id: MailboxId::from_bytes([7; 32]),
            project_name: ShortText::new("provisioned").expect("name"),
            brief: None,
            source,
            destination,
            branch: ShortText::new("feature/exact").expect("branch"),
            base: Some(ShortText::new("main").expect("base")),
            create_branch: true,
        }),
    };
    request.request_digest = project_command_request_digest(&request).expect("digest");
    request
}

fn existing_creation_request() -> ProjectCommandRequest {
    let mut request = ProjectCommandRequest {
        command_id: CommandId::from_bytes([0x11; 32]),
        operation_id: OperationId::from_bytes([0x12; 32]),
        request_digest: CommandDigest::from_bytes([0; 32]),
        account_id: AccountId::from_bytes([0x13; 32]),
        project_id: ProjectId::from_bytes([0x14; 32]),
        home: InstallationId::from_bytes([0x15; 32]),
        expected_head: None,
        issued_at: Timestamp::from_unix_millis(16),
        action: ProjectCommandAction::Create(ProjectCreationRequest {
            mailbox_id: MailboxId::from_bytes([0x17; 32]),
            project_name: ShortText::new("existing").expect("name"),
            brief: None,
            resource_id: ResourceId::from_bytes([0x18; 32]),
            resource: locator("/var/work/existing"),
        }),
    };
    request.request_digest = project_command_request_digest(&request).expect("digest");
    request
}

fn identified_resource(request: &ProjectCommandRequest) -> ProjectResource {
    let ProjectCommandAction::ProvisionWorktree(provisioning) = &request.action else {
        panic!("provisioning request")
    };
    ProjectResource {
        resource_id: ResourceId::from_bytes(hq_projects_test_hash(&[
            b"hq-project-provisioned-resource-v1",
            request.operation_id.as_bytes(),
        ])),
        display_locator: provisioning.destination.clone(),
        canonical_locator: provisioning.destination.clone(),
        health: ResourceHealth::Healthy,
    }
}

fn derived_git_operation(request: &ProjectCommandRequest) -> OperationId {
    let ProjectCommandAction::ProvisionWorktree(provisioning) = &request.action else {
        panic!("provisioning request")
    };
    OperationId::from_bytes(hq_projects_test_hash(&[
        b"hq-project-effect-v1",
        request.operation_id.as_bytes(),
        b"git-worktree",
        provisioning.destination.value().as_bytes(),
    ]))
}

fn hq_projects_test_hash(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().into()
}

fn locator(value: &str) -> ResourceLocator {
    ResourceLocator::new(
        ResourceScheme::WorkingTree,
        BoundedText::new(value).expect("locator"),
    )
}

fn path_locator(path: &Path) -> ResourceLocator {
    locator(path.to_str().expect("UTF-8 test path"))
}

fn initialize_git_repository(repository: &Path) {
    fs::create_dir(repository).expect("repository creates");
    for arguments in [
        vec!["init", "-q"],
        vec!["config", "user.email", "hq@example.invalid"],
        vec!["config", "user.name", "HQ Test"],
    ] {
        run_git(repository, &arguments);
    }
    fs::write(repository.join("tracked.txt"), "initial\n").expect("tracked file writes");
    run_git(repository, &["add", "tracked.txt"]);
    run_git(repository, &["commit", "-qm", "initial"]);
}

fn run_git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("Git runs");
    assert!(output.status.success(), "Git failed: {output:?}");
}

fn unavailable<T>() -> Result<T, ApplicationError> {
    Err(ApplicationError::new(
        ApplicationErrorCode::AdapterUnavailable,
    ))
}

#[allow(dead_code)]
fn domain_error(category: ErrorCategory, code: &'static str) -> DomainError {
    DomainError::new(category, ErrorCode::new(code).expect("code"))
}
