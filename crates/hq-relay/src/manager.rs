//! Sole manager ownership for durable relay policy sessions.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use sha2::{Digest, Sha256};

use crate::{
    MAX_STATE_QUERY_ITEMS, RelayPagePosition, RelayPolicy, RelayPortError, RelaySession,
    RelaySessionConfig, RelaySessionDependencies, RelayStateQuery, RelayUrl,
};

/// Ownership, polling, and policy-scan bounds for the relay manager.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelayManagerConfig {
    /// Per-session state-machine bounds.
    pub session: RelaySessionConfig,
    /// Policies read in each keyset page.
    pub policy_page_items: usize,
    /// Maximum enabled session owners admitted at once.
    pub max_sessions: usize,
    /// Periodic missed-wake repair interval.
    pub periodic_poll: Duration,
}

impl Default for RelayManagerConfig {
    fn default() -> Self {
        Self {
            session: RelaySessionConfig::default(),
            policy_page_items: 64,
            max_sessions: 64,
            periodic_poll: Duration::from_secs(5),
        }
    }
}

impl RelayManagerConfig {
    fn validate(&self) -> Result<(), RelayPortError> {
        if self.policy_page_items == 0
            || self.policy_page_items > MAX_STATE_QUERY_ITEMS
            || self.max_sessions == 0
            || self.periodic_poll.is_zero()
        {
            return Err(RelayPortError::InvalidInput);
        }
        Ok(())
    }
}

/// Bounded stable shutdown evidence for all session owners.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelayManagerReport {
    /// Session workers created across policy generations.
    pub sessions_started: usize,
    /// Session workers completely joined.
    pub sessions_joined: usize,
    /// Stable failures retained up to the configured session bound.
    pub failures: Vec<RelayPortError>,
}

/// Sole owner of policy reconciliation and every relay session worker.
pub struct RelayManager {
    stopping: Arc<AtomicBool>,
    wakes: SyncSender<()>,
    supervisor: Option<JoinHandle<RelayManagerReport>>,
}

impl RelayManager {
    /// Starts policy reconciliation and exactly one worker per enabled relay URL.
    pub fn start(
        config: RelayManagerConfig,
        dependencies: RelaySessionDependencies,
    ) -> Result<Self, RelayPortError> {
        config.validate()?;
        let stopping = Arc::new(AtomicBool::new(false));
        let (wakes, wake_receiver) = mpsc::sync_channel(1);
        let thread_stopping = Arc::clone(&stopping);
        let supervisor = thread::Builder::new()
            .name("hq-relay-manager".to_owned())
            .spawn(move || supervise(&config, &dependencies, &thread_stopping, &wake_receiver))
            .map_err(|_| RelayPortError::Unavailable)?;
        Ok(Self {
            stopping,
            wakes,
            supervisor: Some(supervisor),
        })
    }

    /// Coalesces a durable work or policy-refresh notification without reconnecting sessions.
    pub fn wake(&self) -> Result<(), RelayPortError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(RelayPortError::Unavailable);
        }
        match self.wakes.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => Err(RelayPortError::Unavailable),
        }
    }

    /// Stops intake, closes every session, and joins the complete ownership tree.
    pub fn shutdown(mut self) -> Result<RelayManagerReport, RelayPortError> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> Result<RelayManagerReport, RelayPortError> {
        self.stopping.store(true, Ordering::Release);
        let _ = self.wakes.try_send(());
        self.supervisor
            .take()
            .ok_or(RelayPortError::Unavailable)?
            .join()
            .map_err(|_| RelayPortError::Unavailable)
    }
}

impl Drop for RelayManager {
    fn drop(&mut self) {
        if self.supervisor.is_some() {
            let _ = self.shutdown_inner();
        }
    }
}

struct Worker {
    policy: RelayPolicy,
    recovers_staging: bool,
    stopping: Arc<AtomicBool>,
    wakes: SyncSender<()>,
    join: JoinHandle<Result<(), RelayPortError>>,
}

fn supervise(
    config: &RelayManagerConfig,
    dependencies: &RelaySessionDependencies,
    stopping: &AtomicBool,
    wake_receiver: &Receiver<()>,
) -> RelayManagerReport {
    let mut workers = BTreeMap::<RelayUrl, Worker>::new();
    let mut report = RelayManagerReport::default();
    loop {
        match load_enabled_policies(config, dependencies) {
            Ok(policies) => reconcile(policies, config, dependencies, &mut workers, &mut report),
            Err(error) => retain_failure(config, &mut report, error),
        }
        if stopping.load(Ordering::Acquire) {
            break;
        }
        for worker in workers.values() {
            let _ = worker.wakes.try_send(());
        }
        match wake_receiver.recv_timeout(config.periodic_poll) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    for (_, worker) in workers {
        stop_worker(worker, config, &mut report);
    }
    report
}

fn load_enabled_policies(
    config: &RelayManagerConfig,
    dependencies: &RelaySessionDependencies,
) -> Result<BTreeMap<RelayUrl, RelayPolicy>, RelayPortError> {
    let mut query = policy_query(config.policy_page_items);
    let mut policies = BTreeMap::new();
    loop {
        let page = dependencies.state.load_page(query)?;
        for policy in page.state.policies {
            if policy.enabled {
                if policies.len() == config.max_sessions {
                    return Err(RelayPortError::Backpressure);
                }
                policies.insert(policy.url.clone(), policy);
            }
        }
        let Some(next) = page.next else {
            break;
        };
        query = next;
    }
    Ok(policies)
}

fn reconcile(
    desired: BTreeMap<RelayUrl, RelayPolicy>,
    config: &RelayManagerConfig,
    dependencies: &RelaySessionDependencies,
    workers: &mut BTreeMap<RelayUrl, Worker>,
    report: &mut RelayManagerReport,
) {
    let staging_owner = desired.keys().next().cloned();
    let remove = workers
        .iter()
        .filter_map(|(url, worker)| {
            let changed = desired
                .get(url)
                .is_none_or(|policy| policy != &worker.policy);
            let recovery_changed = worker.recovers_staging != (staging_owner.as_ref() == Some(url));
            (changed || recovery_changed || worker.join.is_finished()).then_some(url.clone())
        })
        .collect::<Vec<_>>();
    for url in remove {
        if let Some(worker) = workers.remove(&url) {
            stop_worker(worker, config, report);
        }
    }
    for (url, policy) in desired {
        if workers.contains_key(&url) {
            continue;
        }
        let recovers_staging = staging_owner.as_ref() == Some(&url);
        match spawn_worker(policy, recovers_staging, config, dependencies) {
            Ok(worker) => {
                report.sessions_started = report.sessions_started.saturating_add(1);
                workers.insert(url, worker);
            }
            Err(error) => retain_failure(config, report, error),
        }
    }
}

fn spawn_worker(
    policy: RelayPolicy,
    recovers_staging: bool,
    config: &RelayManagerConfig,
    dependencies: &RelaySessionDependencies,
) -> Result<Worker, RelayPortError> {
    let stopping = Arc::new(AtomicBool::new(false));
    let (wakes, wake_receiver) = mpsc::sync_channel(1);
    let thread_stopping = Arc::clone(&stopping);
    let thread_policy = policy.clone();
    let mut thread_config = config.clone();
    thread_config.session.recover_staging = recovers_staging;
    let thread_dependencies = dependencies.clone();
    let join = thread::Builder::new()
        .name("hq-relay-session".to_owned())
        .spawn(move || {
            run_worker(
                thread_policy,
                &thread_config,
                &thread_dependencies,
                &thread_stopping,
                &wake_receiver,
            )
        })
        .map_err(|_| RelayPortError::Unavailable)?;
    Ok(Worker {
        policy,
        recovers_staging,
        stopping,
        wakes,
        join,
    })
}

fn run_worker(
    policy: RelayPolicy,
    config: &RelayManagerConfig,
    dependencies: &RelaySessionDependencies,
    stopping: &AtomicBool,
    wake_receiver: &Receiver<()>,
) -> Result<(), RelayPortError> {
    let url = policy.url.clone();
    let mut session = RelaySession::new(policy, config.session.clone(), dependencies.clone())?;
    let identity: [u8; 32] = Sha256::digest(url.as_str().as_bytes()).into();
    let mut failures = 0_u32;
    while !stopping.load(Ordering::Acquire) {
        let tick = session.tick();
        let wait = match tick {
            Ok(progress) => {
                failures = 0;
                if progress.frames == config.session.max_frames_per_tick {
                    continue;
                }
                config.periodic_poll
            }
            Err(
                RelayPortError::Connection
                | RelayPortError::Unavailable
                | RelayPortError::Backpressure,
            ) => {
                let _ = session.close();
                failures = failures.saturating_add(1);
                worker_backoff(config, dependencies, &url, identity, failures)
            }
            Err(error) => {
                let _ = session.close();
                return Err(error);
            }
        };
        match wake_receiver.recv_timeout(wait) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    session.close()
}

fn worker_backoff(
    config: &RelayManagerConfig,
    dependencies: &RelaySessionDependencies,
    url: &RelayUrl,
    identity: [u8; 32],
    failures: u32,
) -> Duration {
    let initial = duration_millis(config.session.retry_initial);
    let maximum = duration_millis(config.session.retry_max);
    let shift = failures.saturating_sub(1).min(63);
    let base = initial.checked_shl(shift).unwrap_or(u64::MAX).min(maximum);
    let jitter = dependencies
        .jitter
        .jitter_millis(url, identity, failures, base / 4);
    Duration::from_millis(base.saturating_add(jitter).min(maximum))
}

fn stop_worker(worker: Worker, config: &RelayManagerConfig, report: &mut RelayManagerReport) {
    worker.stopping.store(true, Ordering::Release);
    let _ = worker.wakes.try_send(());
    match worker.join.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => retain_failure(config, report, error),
        Err(_) => retain_failure(config, report, RelayPortError::Unavailable),
    }
    report.sessions_joined = report.sessions_joined.saturating_add(1);
}

fn retain_failure(
    config: &RelayManagerConfig,
    report: &mut RelayManagerReport,
    error: RelayPortError,
) {
    if report.failures.len() < config.max_sessions {
        report.failures.push(error);
    }
}

fn policy_query(limit: usize) -> RelayStateQuery {
    RelayStateQuery {
        limit,
        policies: RelayPagePosition::Start,
        outbound: RelayPagePosition::Done,
        prepared: RelayPagePosition::Done,
        attempts: RelayPagePosition::Done,
        cursors: RelayPagePosition::Done,
        staged: RelayPagePosition::Done,
        quarantine: RelayPagePosition::Done,
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
