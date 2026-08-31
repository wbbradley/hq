//! Node lifecycle owner for durable project intake and bounded recovery.

use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use hq_application::{
    AgentRetirementOutcome, AgentRetirementRequest, ApplicationError, ApplicationErrorCode,
    AuthoritativeSnapshot, CommitFacts, ControlProjects, EffectOutcome, EffectRequest,
    FactMutation, InspectResource, MutationAttempt, MutationOutcome, ProjectCommandOutcome,
    ProjectCommandRequest, PublishWake, QueryDomain, ResourceInspectionRequest,
    ResourceInspectionResult, RetireAgents,
};
use hq_domain::{Page, PageCursor, Timestamp};
use hq_projects::{
    ApplicationCanonicalProjectPort, ApplicationProjectInputReconciler,
    ApplicationRemoteProjectCommandPort, GitWorktreeAdapter, GitWorktreeAdapterConfig,
    PlanAutomaticProjectDispatches, ProjectCommandRouter, ProjectInputReconciliation,
    ProjectRuntimePort, ProjectWorkerPort, ProjectWorkflowManager, ReconcileProjectInputs,
};
use hq_protocol::Bip340Signer;
use hq_reducer::{AuthorityPolicy, ConversationKey};
use hq_resources::ExecGit;
use hq_store::{Store, StoreGateway};

use crate::{
    CancellationToken, ComponentDrain, ComponentError, NodeComponent, ProjectResourceAdapter,
    ProjectSagaStoreAdapter, RelayNodeComponent,
};

/// Application store capability that schedules relay work after a committed canonical mutation.
#[derive(Clone)]
pub struct WakingApplicationStore<W> {
    store: StoreGateway,
    wake: W,
}

impl<W> WakingApplicationStore<W> {
    /// Binds a durable application store capability to post-commit relay scheduling.
    pub const fn new(store: StoreGateway, wake: W) -> Self {
        Self { store, wake }
    }
}

impl<W> QueryDomain for WakingApplicationStore<W> {
    fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError> {
        self.store.authoritative_snapshot()
    }

    fn conversation_entries(
        &self,
        key: &ConversationKey,
        limit: usize,
        cursor: Option<&PageCursor>,
    ) -> Result<Page<hq_application::ConversationEntry>, ApplicationError> {
        self.store.conversation_entries(key, limit, cursor)
    }
}

impl<W: PublishWake> CommitFacts for WakingApplicationStore<W> {
    fn commit_facts(&self, request: FactMutation) -> Result<MutationAttempt, ApplicationError> {
        let attempt = self.store.commit_facts(request)?;
        if let MutationAttempt::Completed(receipt) = &attempt
            && matches!(receipt.outcome(), MutationOutcome::Committed)
        {
            let _ = self.wake.publish_wake(receipt.revision());
        }
        Ok(attempt)
    }
}

/// Concrete project worker assembled from node-owned durable and external capabilities.
pub type StandardProjectWorker<R> = ProjectCommandRouter<
    ProjectWorkflowManager<
        ProjectSagaStoreAdapter,
        ApplicationCanonicalProjectPort<WakingApplicationStore<RelayNodeComponent>>,
        R,
        ProjectResourceAdapter,
        GitWorktreeAdapter<ExecGit>,
    >,
    ApplicationRemoteProjectCommandPort<WakingApplicationStore<RelayNodeComponent>>,
>;

/// Concrete foreground project component for one injected managed-runtime capability.
pub type StandardProjectNodeComponent<R> = ProjectNodeComponent<
    StandardProjectWorker<R>,
    ProjectResourceAdapter,
    ApplicationProjectInputReconciler<WakingApplicationStore<RelayNodeComponent>>,
>;

/// Result of one bounded project-message sequencing and automatic-dispatch pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectMessageReconciliation {
    /// Human project inputs accepted during this pass.
    pub inputs: ProjectInputReconciliation,
    /// Automatic dispatch commands admitted by the durable project worker.
    pub dispatch_commands: usize,
    /// Whether either sequencing or dispatch planning reached the supplied bound.
    pub truncated: bool,
}

/// Project-owned capability that sequences human messages and starts durable pending dispatch.
pub trait ReconcileProjectMessages {
    /// Reconciles at most `limit` inputs and runnable projects.
    fn reconcile_project_messages(
        &self,
        limit: usize,
    ) -> Result<ProjectMessageReconciliation, ApplicationError>;
}

/// Composes the complete standard project worker without taking store or signer ownership.
pub fn compose_standard_project_component<R: ProjectRuntimePort>(
    config: ProjectNodeConfig,
    store: &Store,
    policy: AuthorityPolicy,
    signer: Arc<Bip340Signer>,
    home: hq_domain::InstallationId,
    runtime: R,
    wake: RelayNodeComponent,
) -> StandardProjectNodeComponent<R> {
    let gateway = WakingApplicationStore::new(StoreGateway::new(store, policy, signer), wake);
    let inputs = ApplicationProjectInputReconciler::new(gateway.clone(), home);
    let resources = ProjectResourceAdapter::system(home);
    let workflow = ProjectWorkflowManager::with_git(
        ProjectSagaStoreAdapter::new(store.project_saga_state_handle()),
        ApplicationCanonicalProjectPort::new(gateway.clone()),
        runtime,
        resources.clone(),
        GitWorktreeAdapter::new(GitWorktreeAdapterConfig::default(), ExecGit::system()),
    );
    let worker = ProjectCommandRouter::new(
        home,
        workflow,
        ApplicationRemoteProjectCommandPort::new(gateway, home),
    );
    ProjectNodeComponent::new(config, worker, resources, inputs)
}

/// Passive bounded project-worker lifecycle configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectNodeConfig {
    /// Maximum local and remote records inspected by each startup or drain repair pass.
    pub recovery_limit: NonZeroUsize,
    /// Explicit semantic receipt time used for home-targeted commands found at startup.
    pub recovery_time: Timestamp,
}

/// Owns project command admission and bounded durable recovery around injected capabilities.
pub struct ProjectNodeComponent<W, F, I> {
    config: ProjectNodeConfig,
    worker: W,
    resources: F,
    inputs: I,
    accepting: AtomicBool,
}

impl<W, F, I> ProjectNodeComponent<W, F, I> {
    /// Owns one complete project worker and its local inspection capability.
    pub const fn new(config: ProjectNodeConfig, worker: W, resources: F, inputs: I) -> Self {
        Self {
            config,
            worker,
            resources,
            inputs,
            accepting: AtomicBool::new(false),
        }
    }

    fn ensure_accepting(&self) -> Result<(), ApplicationError> {
        if self.accepting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ))
        }
    }

    fn repair(&self) -> Result<(), ComponentError>
    where
        W: ProjectWorkerPort,
        I: ReconcileProjectInputs + PlanAutomaticProjectDispatches,
    {
        self.inputs
            .reconcile_project_inputs(self.config.recovery_limit.get())
            .map_err(|_| ComponentError::unavailable())?;
        self.worker
            .repair_pending(self.config.recovery_time, self.config.recovery_limit.get())
            .map_err(|_| ComponentError::unavailable())?;
        self.dispatch_automatic(self.config.recovery_limit.get())
            .map(|_| ())
            .map_err(|_| ComponentError::unavailable())
    }

    fn dispatch_automatic(&self, limit: usize) -> Result<(usize, bool), ApplicationError>
    where
        W: ControlProjects,
        I: PlanAutomaticProjectDispatches,
    {
        let plan = self.inputs.plan_automatic_project_dispatches(limit)?;
        let mut admitted = 0;
        for request in plan.requests {
            let outcome = self.worker.control_project(request)?;
            if !matches!(outcome, ProjectCommandOutcome::Rejected { .. }) {
                admitted += 1;
            }
        }
        Ok((admitted, plan.truncated))
    }

    fn reconcile_messages(
        &self,
        limit: usize,
    ) -> Result<ProjectMessageReconciliation, ApplicationError>
    where
        W: ControlProjects,
        I: ReconcileProjectInputs + PlanAutomaticProjectDispatches,
    {
        let inputs = self.inputs.reconcile_project_inputs(limit)?;
        let (dispatch_commands, dispatch_truncated) = self.dispatch_automatic(limit)?;
        Ok(ProjectMessageReconciliation {
            truncated: inputs.truncated || dispatch_truncated,
            inputs,
            dispatch_commands,
        })
    }
}

impl<W, F, I> std::fmt::Debug for ProjectNodeComponent<W, F, I> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectNodeComponent")
            .field("config", &self.config)
            .field("accepting", &self.accepting.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl<W: ProjectWorkerPort, F, I: ReconcileProjectInputs + PlanAutomaticProjectDispatches>
    NodeComponent for ProjectNodeComponent<W, F, I>
{
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        self.repair()?;
        self.accepting.store(true, Ordering::Release);
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.accepting.store(false, Ordering::Release);
        Ok(())
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        self.repair()?;
        Ok(ComponentDrain::Complete)
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        self.accepting.store(false, Ordering::Release);
        Ok(())
    }
}

impl<W: ProjectWorkerPort, F, I: ReconcileProjectInputs + PlanAutomaticProjectDispatches>
    ControlProjects for ProjectNodeComponent<W, F, I>
{
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.ensure_accepting()?;
        let outcome = self.worker.control_project(request)?;
        self.reconcile_messages(self.config.recovery_limit.get())?;
        Ok(outcome)
    }
}

impl<W: ProjectWorkerPort + RetireAgents, F, I> RetireAgents for ProjectNodeComponent<W, F, I> {
    fn retire_agent(
        &self,
        request: AgentRetirementRequest,
    ) -> Result<AgentRetirementOutcome, ApplicationError> {
        self.ensure_accepting()?;
        self.worker.retire_agent(request)
    }
}

impl<W, F: InspectResource, I> InspectResource for ProjectNodeComponent<W, F, I> {
    fn inspect_resource(
        &self,
        request: &EffectRequest<ResourceInspectionRequest>,
    ) -> Result<EffectOutcome<ResourceInspectionResult>, ApplicationError> {
        self.ensure_accepting()?;
        self.resources.inspect_resource(request)
    }
}

impl<W, F, I: ReconcileProjectInputs> ReconcileProjectInputs for ProjectNodeComponent<W, F, I> {
    fn reconcile_project_inputs(
        &self,
        limit: usize,
    ) -> Result<hq_projects::ProjectInputReconciliation, ApplicationError> {
        self.ensure_accepting()?;
        self.inputs.reconcile_project_inputs(limit)
    }
}

impl<W: ProjectWorkerPort, F, I: ReconcileProjectInputs + PlanAutomaticProjectDispatches>
    ReconcileProjectMessages for ProjectNodeComponent<W, F, I>
{
    fn reconcile_project_messages(
        &self,
        limit: usize,
    ) -> Result<ProjectMessageReconciliation, ApplicationError> {
        self.ensure_accepting()?;
        self.reconcile_messages(limit)
    }
}
