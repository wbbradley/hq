//! Node lifecycle owner for durable project intake and bounded recovery.

use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use hq_application::{
    AgentRetirementOutcome, AgentRetirementRequest, ApplicationError, ApplicationErrorCode,
    AuthoritativeConversationView, AuthoritativeSnapshot, CommitFacts, ControlProjects,
    ConversationPageSelection, EffectOutcome, EffectRequest, FactMutation, InspectResource,
    MutationAttempt, MutationOutcome, ProjectCommandOutcome, ProjectCommandRequest, PublishWake,
    QueryDomain, ResourceInspectionRequest, ResourceInspectionResult, RetireAgents,
};
use hq_domain::{Page, PageCursor, Timestamp};
use hq_projects::{
    ApplicationCanonicalProjectPort, ApplicationProjectInputReconciler,
    ApplicationRemoteProjectCommandPort, GitWorktreeAdapter, GitWorktreeAdapterConfig,
    PlanAutomaticProjectCommands, ProjectCommandRouter, ProjectInputReconciliation,
    ProjectRuntimePort, ProjectWorkerPort, ProjectWorkflowManager, ReconcileProjectInputs,
};
use hq_protocol::Bip340Signer;
use hq_reducer::{AuthorityPolicy, ConversationKey};
use hq_resources::ExecGit;
use hq_store::{RevisionInvalidations, Store, StoreGateway};
use tokio::{runtime::Builder, sync::watch};

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

    fn authoritative_conversation_view(
        &self,
        selection: Option<&ConversationPageSelection>,
    ) -> Result<AuthoritativeConversationView, ApplicationError> {
        self.store.authoritative_conversation_view(selection)
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

/// Result of one bounded project-message sequencing and automatic-command pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectMessageReconciliation {
    /// Human project inputs accepted during this pass.
    pub inputs: ProjectInputReconciliation,
    /// Automatic resume or dispatch commands admitted by the durable project worker.
    pub automatic_commands: usize,
    /// Whether either sequencing or automatic planning reached the supplied bound.
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

/// Project-owned nonblocking signal that schedules reconciliation from durable state.
pub trait ScheduleProjectReconciliation {
    /// Coalesces one request for a background reconciliation pass.
    fn schedule_project_reconciliation(&self);
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
    ProjectNodeComponent::new_with_invalidations(
        config,
        worker,
        resources,
        inputs,
        store.subscribe_invalidations(),
    )
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
    worker: Arc<Mutex<W>>,
    resources: F,
    inputs: Arc<Mutex<I>>,
    accepting: AtomicBool,
    reconciliation: watch::Sender<ProjectReconciliationSignal>,
    reconciliation_observer: Option<watch::Receiver<ProjectReconciliationSignal>>,
    store_invalidations: Option<RevisionInvalidations>,
    reconciliation_task: Option<JoinHandle<Result<(), ComponentError>>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProjectReconciliationSignal {
    generation: u64,
    stopping: bool,
    force_stop: bool,
}

impl<W, F, I> ProjectNodeComponent<W, F, I> {
    /// Owns one complete project worker and its local inspection capability.
    pub fn new(config: ProjectNodeConfig, worker: W, resources: F, inputs: I) -> Self {
        Self::new_inner(config, worker, resources, inputs, None)
    }

    /// Owns a project worker subscribed to committed store revisions before startup.
    pub fn new_with_invalidations(
        config: ProjectNodeConfig,
        worker: W,
        resources: F,
        inputs: I,
        store_invalidations: RevisionInvalidations,
    ) -> Self {
        Self::new_inner(config, worker, resources, inputs, Some(store_invalidations))
    }

    fn new_inner(
        config: ProjectNodeConfig,
        worker: W,
        resources: F,
        inputs: I,
        store_invalidations: Option<RevisionInvalidations>,
    ) -> Self {
        let (reconciliation, reconciliation_observer) =
            watch::channel(ProjectReconciliationSignal::default());
        Self {
            config,
            worker: Arc::new(Mutex::new(worker)),
            resources,
            inputs: Arc::new(Mutex::new(inputs)),
            accepting: AtomicBool::new(false),
            reconciliation,
            reconciliation_observer: Some(reconciliation_observer),
            store_invalidations,
            reconciliation_task: None,
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
        I: ReconcileProjectInputs + PlanAutomaticProjectCommands,
    {
        lock_capability(&self.inputs)
            .map_err(|_| ComponentError::unavailable())?
            .reconcile_project_inputs(self.config.recovery_limit.get())
            .map_err(|_| ComponentError::unavailable())?;
        lock_capability(&self.worker)
            .map_err(|_| ComponentError::unavailable())?
            .repair_pending(self.config.recovery_time, self.config.recovery_limit.get())
            .map_err(|_| ComponentError::unavailable())?;
        self.submit_automatic(self.config.recovery_limit.get())
            .map(|_| ())
            .map_err(|_| ComponentError::unavailable())
    }

    fn submit_automatic(&self, limit: usize) -> Result<(usize, bool), ApplicationError>
    where
        W: ControlProjects,
        I: PlanAutomaticProjectCommands,
    {
        let plan = lock_capability(&self.inputs)?.plan_automatic_project_commands(limit)?;
        let mut admitted = 0;
        for request in plan.requests {
            let outcome = lock_capability(&self.worker)?.control_project(request)?;
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
        I: ReconcileProjectInputs + PlanAutomaticProjectCommands,
    {
        let inputs = lock_capability(&self.inputs)?.reconcile_project_inputs(limit)?;
        let (automatic_commands, commands_truncated) = self.submit_automatic(limit)?;
        Ok(ProjectMessageReconciliation {
            truncated: inputs.truncated || commands_truncated,
            inputs,
            automatic_commands,
        })
    }

    fn stop_reconciliation(&mut self, force_stop: bool) -> Result<(), ComponentError> {
        self.reconciliation.send_modify(|state| {
            state.stopping = true;
            state.force_stop = force_stop;
            state.generation = state.generation.wrapping_add(1);
        });
        let Some(task) = self.reconciliation_task.take() else {
            return Ok(());
        };
        task.join().map_err(|_| ComponentError::unavailable())?
    }
}

fn lock_capability<T>(capability: &Mutex<T>) -> Result<MutexGuard<'_, T>, ApplicationError> {
    capability
        .lock()
        .map_err(|_| ApplicationError::new(ApplicationErrorCode::AdapterUnavailable))
}

fn reconcile_shared<W, I>(
    worker: &Mutex<W>,
    inputs: &Mutex<I>,
    limit: usize,
) -> Result<ProjectMessageReconciliation, ApplicationError>
where
    W: ControlProjects,
    I: ReconcileProjectInputs + PlanAutomaticProjectCommands,
{
    let inputs_outcome = lock_capability(inputs)?.reconcile_project_inputs(limit)?;
    let plan = lock_capability(inputs)?.plan_automatic_project_commands(limit)?;
    let mut admitted = 0;
    for request in plan.requests {
        let outcome = lock_capability(worker)?.control_project(request)?;
        if !matches!(outcome, ProjectCommandOutcome::Rejected { .. }) {
            admitted += 1;
        }
    }
    Ok(ProjectMessageReconciliation {
        truncated: inputs_outcome.truncated || plan.truncated,
        inputs: inputs_outcome,
        automatic_commands: admitted,
    })
}

async fn run_project_reconciliation<W, I>(
    worker: &Mutex<W>,
    inputs: &Mutex<I>,
    mut control: watch::Receiver<ProjectReconciliationSignal>,
    mut store_invalidations: Option<RevisionInvalidations>,
    scheduler: &watch::Sender<ProjectReconciliationSignal>,
    limit: usize,
    recovery_time: Timestamp,
) -> Result<(), ComponentError>
where
    W: ProjectWorkerPort,
    I: ReconcileProjectInputs + PlanAutomaticProjectCommands,
{
    loop {
        match store_invalidations.as_mut() {
            Some(invalidations) => {
                tokio::select! {
                    changed = control.changed() => {
                        if changed.is_err() {
                            return Ok(());
                        }
                    }
                    revision = invalidations.next_revision() => {
                        if revision.is_none() {
                            store_invalidations = None;
                            continue;
                        }
                    }
                }
            }
            None => {
                if control.changed().await.is_err() {
                    return Ok(());
                }
            }
        }
        let signal = *control.borrow_and_update();
        if signal.force_stop {
            return Ok(());
        }
        let reconciliation = lock_capability(worker)
            .and_then(|worker| worker.repair_pending(recovery_time, limit))
            .and_then(|_| reconcile_shared(worker, inputs, limit))
            .and_then(|first| {
                if first.inputs.accepted == 0 && !first.truncated {
                    // The durable mailbox receipt can become visible just ahead of its projection.
                    // One immediate bounded reread crosses that actor handoff without retrying the
                    // mutation or carrying message data in the wake.
                    reconcile_shared(worker, inputs, limit)
                } else {
                    Ok(first)
                }
            });
        match reconciliation {
            Ok(outcome) if outcome.truncated => {
                scheduler.send_modify(|state| {
                    state.generation = state.generation.wrapping_add(1);
                });
            }
            outcome if signal.stopping => {
                return outcome
                    .map(|_| ())
                    .map_err(|_| ComponentError::unavailable());
            }
            Ok(_) | Err(_) => {}
        }
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

impl<W: ProjectWorkerPort, F, I: ReconcileProjectInputs + PlanAutomaticProjectCommands>
    NodeComponent for ProjectNodeComponent<W, F, I>
where
    W: Send + 'static,
    I: Send + 'static,
{
    fn start(&mut self, _cancellation: CancellationToken) -> Result<(), ComponentError> {
        self.repair()?;
        let observer = self
            .reconciliation_observer
            .take()
            .ok_or_else(ComponentError::unavailable)?;
        let store_invalidations = self.store_invalidations.take();
        let runtime = Builder::new_current_thread()
            .build()
            .map_err(|_| ComponentError::unavailable())?;
        let worker = Arc::clone(&self.worker);
        let inputs = Arc::clone(&self.inputs);
        let scheduler = self.reconciliation.clone();
        let limit = self.config.recovery_limit.get();
        let recovery_time = self.config.recovery_time;
        self.reconciliation_task = Some(
            thread::Builder::new()
                .name("hq-project-reconciliation".to_owned())
                .spawn(move || {
                    runtime.block_on(run_project_reconciliation(
                        &worker,
                        &inputs,
                        observer,
                        store_invalidations,
                        &scheduler,
                        limit,
                        recovery_time,
                    ))
                })
                .map_err(|_| ComponentError::unavailable())?,
        );
        self.accepting.store(true, Ordering::Release);
        Ok(())
    }

    fn stop_intake(&mut self) -> Result<(), ComponentError> {
        self.accepting.store(false, Ordering::Release);
        Ok(())
    }

    fn drain(&mut self) -> Result<ComponentDrain, ComponentError> {
        self.stop_reconciliation(false)?;
        Ok(ComponentDrain::Complete)
    }

    fn force_stop(&mut self) -> Result<(), ComponentError> {
        self.accepting.store(false, Ordering::Release);
        self.stop_reconciliation(true)
    }
}

impl<W: ProjectWorkerPort, F, I: ReconcileProjectInputs + PlanAutomaticProjectCommands>
    ControlProjects for ProjectNodeComponent<W, F, I>
{
    fn control_project(
        &self,
        request: ProjectCommandRequest,
    ) -> Result<ProjectCommandOutcome, ApplicationError> {
        self.ensure_accepting()?;
        let outcome = lock_capability(&self.worker)?.control_project(request)?;
        self.schedule_project_reconciliation();
        Ok(outcome)
    }
}

impl<W: ProjectWorkerPort + RetireAgents, F, I> RetireAgents for ProjectNodeComponent<W, F, I> {
    fn retire_agent(
        &self,
        request: AgentRetirementRequest,
    ) -> Result<AgentRetirementOutcome, ApplicationError> {
        self.ensure_accepting()?;
        lock_capability(&self.worker)?.retire_agent(request)
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
        lock_capability(&self.inputs)?.reconcile_project_inputs(limit)
    }
}

impl<W: ProjectWorkerPort, F, I: ReconcileProjectInputs + PlanAutomaticProjectCommands>
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

impl<W, F, I> ScheduleProjectReconciliation for ProjectNodeComponent<W, F, I> {
    fn schedule_project_reconciliation(&self) {
        if !self.accepting.load(Ordering::Acquire) {
            return;
        }
        self.reconciliation.send_modify(|state| {
            if !state.stopping {
                state.generation = state.generation.wrapping_add(1);
            }
        });
    }
}
